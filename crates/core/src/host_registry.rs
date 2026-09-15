use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Context};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const HOST_MIGRATION_0001: &str = include_str!("../host_migrations/0001_host_registry.sql");
const HOST_MIGRATION_0002: &str =
    include_str!("../host_migrations/0002_sync_protocol_foundation.sql");
const HOST_MIGRATION_0003: &str = include_str!("../host_migrations/0003_plugin_runtime.sql");
const HOST_MIGRATION_0004: &str =
    include_str!("../host_migrations/0004_provider_root_discovery.sql");
const HOST_MIGRATION_0005: &str = include_str!("../host_migrations/0005_sync_orchestration.sql");
const HOST_MIGRATION_0006: &str =
    include_str!("../host_migrations/0006_sync_orchestration_recovery.sql");
const HOST_MIGRATION_0007: &str =
    include_str!("../host_migrations/0007_onedrive_auth_configuration.sql");
const HOST_MIGRATION_0008: &str =
    include_str!("../host_migrations/0008_plugin_credential_retirement.sql");
const HOST_MIGRATION_0009: &str =
    include_str!("../host_migrations/0009_plugin_auth_cancellation.sql");
const HOST_MIGRATION_0010: &str = include_str!("../host_migrations/0010_plugin_secret_handles.sql");
const HOST_MIGRATION_0011: &str =
    include_str!("../host_migrations/0011_plugin_secret_creations.sql");
const HOST_MIGRATION_0012: &str = include_str!("../host_migrations/0012_sync_root_bindings.sql");
const HOST_MIGRATION_0013: &str =
    include_str!("../host_migrations/0013_sync_root_reenable_generation.sql");
const HOST_MIGRATION_0014: &str = include_str!("../host_migrations/0014_sync_root_preflights.sql");
const HOST_MIGRATION_0015: &str = include_str!("../host_migrations/0015_sync_root_recovery.sql");
const HOST_MIGRATION_0016: &str = include_str!("../host_migrations/0016_remote_root_inventory.sql");
const HOST_MIGRATION_0017: &str =
    include_str!("../host_migrations/0017_restore_action_contracts.sql");
const HOST_MIGRATION_0018: &str = include_str!("../host_migrations/0018_descriptor_snapshots.sql");

pub const ONEDRIVE_SECRET_NAMESPACE: &str = "microsoft-onedrive";
pub const ONEDRIVE_CREDENTIAL_PURPOSE: &str = "oauth-v1";

fn cache_remote_descriptor(
    transaction: &rusqlite::Transaction<'_>,
    account_uuid: &str,
    account_generation: i64,
    incoming: &DiscoveredRemoteLibrary,
) -> anyhow::Result<()> {
    let (library_uuid, descriptor_key) =
        crate::sync::descriptor::split_root_key(&incoming.metadata.key)?;
    let mut selected = incoming.clone();
    if let Some(evidence) = &incoming.evidence {
        transaction.execute("INSERT INTO provider_verified_descriptors(account_uuid,library_uuid,descriptor_key,account_generation,candidate_json) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(account_uuid,library_uuid,descriptor_key) DO UPDATE SET account_generation=excluded.account_generation,candidate_json=excluded.candidate_json",params![account_uuid,library_uuid,descriptor_key,account_generation,serde_json::to_string(incoming)?])?;
        transaction.execute("DELETE FROM provider_remote_descriptor_heads WHERE account_uuid=?1 AND library_uuid=?2 AND account_generation<>?3", params![account_uuid,library_uuid,account_generation])?;
        let mut statement = transaction.prepare("SELECT descriptor_key,candidate_json FROM provider_remote_descriptor_heads WHERE account_uuid=?1 AND library_uuid=?2 ORDER BY descriptor_key LIMIT 65")?;
        let rows = statement
            .query_map(params![account_uuid, library_uuid], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        if rows.len() > 64 {
            bail!("descriptor causal head limit exceeded");
        }
        let mut heads = Vec::new();
        let mut dominated = false;
        for (key, json) in rows {
            let existing: DiscoveredRemoteLibrary = serde_json::from_str(&json)?;
            let previous = existing
                .evidence
                .as_ref()
                .context("cached descriptor evidence missing")?;
            let (covers, covered) = crate::sync::descriptor::causal_relation(incoming, &existing)?;
            if covered && (!covers || evidence.truth_sha256 == previous.truth_sha256) {
                // Equivalent truth keeps a deterministic immutable identity, preferring
                // a snapshot over its legacy seed without using arrival or wall time.
                let prefer_incoming =
                    covers && key == crate::sync::descriptor::LEGACY_KEY && descriptor_key != key;
                if !prefer_incoming {
                    dominated = true;
                    heads.push(existing);
                    continue;
                }
            }
            if covers && (!covered || evidence.truth_sha256 == previous.truth_sha256) {
                transaction.execute("DELETE FROM provider_remote_descriptor_heads WHERE account_uuid=?1 AND library_uuid=?2 AND descriptor_key=?3", params![account_uuid,library_uuid,key])?;
            } else {
                heads.push(existing);
            }
        }
        if !dominated {
            transaction.execute("INSERT INTO provider_remote_descriptor_heads(account_uuid,library_uuid,descriptor_key,account_generation,candidate_json) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(account_uuid,library_uuid,descriptor_key) DO UPDATE SET candidate_json=excluded.candidate_json,account_generation=excluded.account_generation", params![account_uuid,library_uuid,descriptor_key,account_generation,serde_json::to_string(incoming)?])?;
            heads.push(incoming.clone());
        }
        if heads.len() != 1 {
            transaction.execute("UPDATE provider_remote_libraries SET compatible=0,descriptor_error_code='descriptor-causal-conflict' WHERE account_uuid=?1 AND library_uuid=?2",params![account_uuid,library_uuid])?;
            transaction.execute("UPDATE provider_remote_sync_roots SET compatible=0,error_code='descriptor-causal-conflict' WHERE account_uuid=?1 AND library_uuid=?2",params![account_uuid,library_uuid])?;
            return Ok(());
        }
        selected = heads.remove(0);
    }
    let metadata = &selected.metadata;
    let (_, descriptor_key) = crate::sync::descriptor::split_root_key(&metadata.key)?;
    let now = unix_timestamp_millis();
    transaction.execute(
        "INSERT INTO provider_remote_libraries(account_uuid,library_uuid,descriptor_size,descriptor_sha256,descriptor_etag,account_generation,updated_at,name,icon,media_count,logical_bytes,last_synced_at,compatible,descriptor_error_code,descriptor_key,checkpoint_key,checkpoint_sha256) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) ON CONFLICT(account_uuid,library_uuid) DO UPDATE SET descriptor_size=excluded.descriptor_size,descriptor_sha256=excluded.descriptor_sha256,descriptor_etag=excluded.descriptor_etag,account_generation=excluded.account_generation,updated_at=excluded.updated_at,name=excluded.name,icon=excluded.icon,media_count=excluded.media_count,logical_bytes=excluded.logical_bytes,last_synced_at=excluded.last_synced_at,compatible=excluded.compatible,descriptor_error_code=excluded.descriptor_error_code,descriptor_key=excluded.descriptor_key,checkpoint_key=excluded.checkpoint_key,checkpoint_sha256=excluded.checkpoint_sha256",
        params![account_uuid,library_uuid,i64::try_from(metadata.size)?,metadata.sha256.as_slice(),metadata.etag,account_generation,now,selected.name,selected.icon,selected.media_count.map(i64::try_from).transpose()?,selected.logical_bytes.map(i64::try_from).transpose()?,selected.last_synced_at,selected.compatible as i64,selected.error_code,descriptor_key,selected.evidence.as_ref().map(|v|&v.checkpoint_key),selected.evidence.as_ref().map(|v|&v.checkpoint_sha256)],
    )?;
    transaction.execute(
        "DELETE FROM provider_remote_sync_roots WHERE account_uuid=?1 AND library_uuid=?2",
        params![account_uuid, library_uuid],
    )?;
    if selected.compatible {
        for root in &selected.roots {
            if uuid::Uuid::parse_str(&root.root_uuid)?.to_string() != root.root_uuid
                || uuid::Uuid::parse_str(&root.root_folder_uuid)?.to_string()
                    != root.root_folder_uuid
                || crate::sync::media::parse_hash(&root.materialization_fingerprint).is_err()
            {
                bail!("invalid remote root descriptor");
            }
            transaction.execute("INSERT INTO provider_remote_sync_roots(account_uuid,library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialization_fingerprint,logical_revision,logical_bytes,compatible,error_code,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,NULL,?10)",params![account_uuid,library_uuid,root.root_uuid,root.root_folder_uuid,root.requested_name,root.materialized_name,root.materialization_fingerprint,i64::try_from(root.logical_revision)?,root.logical_bytes.map(i64::try_from).transpose()?,now])?;
        }
    }
    Ok(())
}

fn enqueue_onedrive_secret_retirements(
    transaction: &rusqlite::Transaction<'_>,
    plugin_id: &str,
    account_uuid: &str,
    primary_secret_ref: Option<&str>,
    now: i64,
) -> anyhow::Result<()> {
    if let Some(secret_ref) = primary_secret_ref {
        transaction.execute("INSERT OR IGNORE INTO plugin_credential_retirements(retirement_uuid,plugin_id,account_uuid,secret_ref,namespace,purpose,attempts,next_retry_at,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,0,?7,?7,?7)", params![uuid::Uuid::now_v7().to_string(),plugin_id,account_uuid,secret_ref,ONEDRIVE_SECRET_NAMESPACE,ONEDRIVE_CREDENTIAL_PURPOSE,now])?;
    }
    transaction.execute("INSERT OR IGNORE INTO plugin_credential_retirements(retirement_uuid,plugin_id,account_uuid,secret_ref,namespace,purpose,attempts,next_retry_at,created_at,updated_at) SELECT lower(hex(randomblob(16))),plugin_id,account_uuid,secret_ref,namespace,purpose,0,?1,?1,?1 FROM plugin_secret_handles WHERE plugin_id=?2 AND account_uuid=?3", params![now, plugin_id, account_uuid])?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProviderStatus {
    pub plugin_id: String,
    pub account_uuid: String,
    pub account_display_name: Option<String>,
    pub account_kind: Option<String>,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredRemoteLibrary {
    pub metadata: super::sync::provider::ObjectMetadata,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub media_count: Option<u64>,
    pub logical_bytes: Option<u64>,
    pub last_synced_at: Option<i64>,
    pub compatible: bool,
    pub error_code: Option<String>,
    pub roots: Vec<DiscoveredRemoteRoot>,
    #[serde(default)]
    pub evidence: Option<crate::sync::descriptor::DescriptorEvidence>,
    #[serde(default)]
    pub history: BTreeMap<String, String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredRemoteRoot {
    pub root_uuid: String,
    pub root_folder_uuid: String,
    pub requested_name: String,
    pub materialized_name: String,
    pub materialization_fingerprint: String,
    pub logical_revision: u64,
    #[serde(default)]
    pub logical_bytes: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLibraryRootSummary {
    pub account_uuid: String,
    pub library_uuid: String,
    pub root_uuid: String,
    pub root_folder_uuid: String,
    pub requested_name: String,
    pub materialized_name: String,
    pub materialization_fingerprint: String,
    pub logical_revision: u64,
    pub logical_bytes: Option<u64>,
    pub proposed_local_path: Option<String>,
    pub selected_by_default: bool,
    pub compatible: bool,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLibraryRootInventory {
    pub items: Vec<RemoteLibraryRootSummary>,
    pub inventory_fence: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRestoreRootResult {
    pub library_uuid: String,
    pub root_uuid: String,
    pub selected: bool,
    pub ok: bool,
    pub reason: Option<String>,
    pub normalized_target_path: Option<String>,
    pub required_bytes: u64,
    pub available_bytes: Option<u64>,
    pub volume_key: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRestoreVolumeResult {
    pub volume_key: String,
    pub required_bytes: u64,
    pub available_bytes: Option<u64>,
    pub ok: bool,
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
    pub preflight_token: Option<String>,
    pub expires_at: i64,
    pub total_items: u64,
    pub total_bytes: u64,
    pub ok: bool,
    pub root_results: Vec<SyncRestoreRootResult>,
    pub volume_results: Vec<SyncRestoreVolumeResult>,
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

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct StartRestoreRequest<'a> {
    pub operation_uuid: &'a str,
    pub account_uuid: &'a str,
    pub library_uuid: &'a str,
    pub managed_root: &'a Path,
    pub restore_mode: &'a str,
    pub inventory_fence: Option<&'a str>,
    pub request_hash: &'a str,
}

#[derive(Debug, Clone)]
pub struct RestoreRootPlanInput {
    pub root_uuid: String,
    pub root_folder_uuid: String,
    pub requested_name: String,
    pub materialized_name: String,
    pub materialization_fingerprint: String,
    pub logical_revision: u64,
    pub logical_bytes: Option<u64>,
    pub selected_binding: Option<NewLocalRootBinding>,
}

#[derive(Debug, Clone)]
pub struct RestoreRootCleanup {
    pub root_uuid: String,
    pub root_folder_uuid: String,
    pub target_path: PathBuf,
    pub local_binding_uuid: String,
    pub binding_generation: u64,
}

#[derive(Debug, Clone)]
pub struct RestorePlanInput {
    pub account_uuid: String,
    pub library_uuid: String,
    pub managed_root: PathBuf,
    pub restore_mode: String,
    pub inventory_fence: Option<String>,
    pub roots: Vec<RestoreRootPlanInput>,
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
    pub name: String,
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
    pub account_kind: Option<String>,
    pub backoff_until: Option<i64>,
    pub last_healthy_at: Option<i64>,
    pub last_error_code: Option<String>,
    pub last_error_detail: Option<String>,
    pub compatibility_reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveConfiguration {
    pub source: String,
    pub configured: bool,
    pub read_only: bool,
    pub revision: u64,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveConfigurationPlan {
    pub plan_uuid: String,
    pub expected_revision: u64,
    pub effective_changed: bool,
    pub affected_libraries: u64,
    pub requires_confirmation: bool,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct CredentialRetirement {
    pub retirement_uuid: String,
    pub plugin_id: String,
    pub account_uuid: String,
    pub secret_ref: String,
    pub namespace: String,
    pub purpose: String,
    pub attempts: u32,
}

#[derive(Debug)]
pub struct OneDriveConfigurationCommit {
    pub fences: Vec<BindingFence>,
    pub effective_changed: bool,
}

const ONEDRIVE_PLUGIN_ID: &str = "com.megle.onedrive";

fn normalized_client_id(value: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let parsed =
        uuid::Uuid::parse_str(value).map_err(|_| anyhow::anyhow!("invalid-onedrive-client-id"))?;
    if parsed.is_nil() {
        anyhow::bail!("invalid-onedrive-client-id");
    }
    Ok(Some(parsed.hyphenated().to_string()))
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

fn restore_plan_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncRestorePlan> {
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
        media_items_total: row
            .get::<_, Option<i64>>(10)?
            .map(|value| value.max(0) as u64),
        media_bytes_done: row.get::<_, i64>(11)?.max(0) as u64,
        media_bytes_total: row
            .get::<_, Option<i64>>(12)?
            .map(|value| value.max(0) as u64),
        error_code: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn restore_plans_for_operation_in_transaction(
    transaction: &rusqlite::Connection,
    operation_uuid: &str,
) -> anyhow::Result<Vec<SyncRestorePlan>> {
    let mut statement = transaction.prepare("SELECT restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,state,phase,catalog_ready,media_items_done,media_items_total,media_bytes_done,media_bytes_total,error_code,updated_at FROM sync_restore_plans WHERE operation_uuid=?1 ORDER BY created_at,restore_uuid")?;
    let plans = statement
        .query_map([operation_uuid], restore_plan_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::from)?;
    Ok(plans)
}

#[allow(dead_code)]
fn root_operation_record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RootOperationRecord> {
    let expected_operation_revision: i64 = row.get(8)?;
    let expected_binding_generation: Option<i64> = row.get(9)?;
    let operation_revision: i64 = row.get(10)?;
    Ok(RootOperationRecord {
        operation_uuid: row.get(0)?,
        library_uuid: row.get(1)?,
        root_uuid: row.get(2)?,
        kind: row.get(3)?,
        state: row.get(4)?,
        phase: row.get(5)?,
        request_hash: row.get(6)?,
        request_json: row.get(7)?,
        expected_operation_revision: expected_operation_revision.try_into().map_err(|_| {
            rusqlite::Error::IntegralValueOutOfRange(8, expected_operation_revision)
        })?,
        expected_binding_generation: expected_binding_generation
            .map(|value| {
                value
                    .try_into()
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(9, value))
            })
            .transpose()?,
        operation_revision: operation_revision
            .try_into()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(10, operation_revision))?,
        cutover_committed: row.get::<_, i64>(11)? != 0,
        error_code: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

pub(crate) fn relocation_resume_state(request_json: &str) -> &'static str {
    if serde_json::from_str::<serde_json::Value>(request_json)
        .ok()
        .and_then(|value| {
            value
                .get("resumeState")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .as_deref()
        == Some("paused")
    {
        "paused"
    } else {
        "active"
    }
}

fn relocation_local_binding_uuid(request_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(request_json)
        .ok()
        .and_then(|value| {
            value
                .get("localBindingUuid")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
}

fn root_publish_as_new_record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RootPublishAsNewRecord> {
    Ok(RootPublishAsNewRecord {
        operation_uuid: row.get(0)?,
        enrollment_operation_uuid: row.get(1)?,
        library_uuid: row.get(2)?,
        source_root_uuid: row.get(3)?,
        new_root_uuid: row.get(4)?,
        new_root_folder_uuid: row.get(5)?,
        new_local_binding_uuid: row.get(6)?,
        root_capture_epoch: row.get(7)?,
        root_actor_epoch: row.get(8)?,
    })
}

fn local_root_binding_record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LocalRootBindingRecord> {
    let binding_generation: i64 = row.get(8)?;
    let operation_revision: i64 = row.get(15)?;
    Ok(LocalRootBindingRecord {
        library_uuid: row.get(0)?,
        root_uuid: row.get(1)?,
        local_binding_uuid: row.get(2)?,
        local_path: row.get(3)?,
        canonical_path_key: row.get(4)?,
        placement_parent: row.get(5)?,
        placement_parent_key: row.get(6)?,
        state: row.get(7)?,
        binding_generation: binding_generation
            .try_into()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(8, binding_generation))?,
        root_capture_epoch: row.get(9)?,
        root_actor_epoch: row.get(10)?,
        desired_materialized_name: row.get(11)?,
        desired_materialization_fingerprint: row.get(12)?,
        applied_materialized_name: row.get(13)?,
        applied_materialization_fingerprint: row.get(14)?,
        operation_revision: operation_revision
            .try_into()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(15, operation_revision))?,
        last_error_code: row.get(16)?,
    })
}

fn remote_sync_root_record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RemoteSyncRootRecord> {
    let logical_revision: i64 = row.get(7)?;
    let logical_bytes: Option<i64> = row.get(10)?;
    let inventory_revision: i64 = row.get(12)?;
    let operation_revision: i64 = row.get(13)?;
    Ok(RemoteSyncRootRecord {
        library_uuid: row.get(0)?,
        library_root_uuid: row.get(1)?,
        root_uuid: row.get(2)?,
        root_folder_uuid: row.get(3)?,
        requested_name: row.get(4)?,
        materialized_name: row.get(5)?,
        materialization_fingerprint: row.get(6)?,
        logical_revision: logical_revision
            .try_into()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(7, logical_revision))?,
        lifecycle: row.get(8)?,
        inventory_state: row.get(9)?,
        logical_bytes: logical_bytes
            .map(|v| {
                v.try_into()
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(10, v))
            })
            .transpose()?,
        checkpoint_uuid: row.get(11)?,
        inventory_revision: inventory_revision
            .try_into()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(12, inventory_revision))?,
        operation_revision: operation_revision
            .try_into()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(13, operation_revision))?,
    })
}

fn reconciliation_apply_order(
    snapshot: &crate::sync_folders::RootSnapshot,
    entity_uuid: &str,
    field_name: &str,
    ordinal: u64,
) -> u64 {
    if field_name == "existence" {
        let depth = reconciliation_entity_depth(snapshot, entity_uuid);
        let deleted = snapshot
            .entities
            .get(entity_uuid)
            .and_then(|fields| fields.get("existence"))
            .is_some_and(|value| value == "deleted");
        if deleted {
            return 3_000_000u64
                .saturating_sub(depth.saturating_mul(1_000))
                .saturating_add(ordinal);
        }
        return depth.saturating_mul(1_000).saturating_add(ordinal);
    }
    if field_name == "location" {
        return 1_000_000u64
            .saturating_add(
                reconciliation_entity_depth(snapshot, entity_uuid).saturating_mul(1_000),
            )
            .saturating_add(ordinal);
    }
    2_000_000u64.saturating_add(ordinal)
}

fn reconciliation_entity_depth(
    snapshot: &crate::sync_folders::RootSnapshot,
    entity_uuid: &str,
) -> u64 {
    let mut depth = 0u64;
    let mut current = entity_uuid.to_owned();
    let mut seen = std::collections::BTreeSet::new();
    while seen.insert(current.clone()) {
        let Some(location) = snapshot
            .entities
            .get(&current)
            .and_then(|fields| fields.get("location"))
        else {
            break;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(location) else {
            break;
        };
        let Some(parent) = value.get("parentUuid").and_then(serde_json::Value::as_str) else {
            break;
        };
        depth = depth.saturating_add(1);
        current = parent.to_owned();
    }
    depth
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

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRootTransport {
    pub plugin_id: String,
    pub account_uuid: String,
    pub account_generation: i64,
    pub runtime_session_id: String,
    pub cursor: Option<String>,
    pub page_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRootBindingRecord {
    pub library_uuid: String,
    pub root_uuid: String,
    pub local_binding_uuid: String,
    pub local_path: String,
    pub canonical_path_key: String,
    pub placement_parent: String,
    pub placement_parent_key: String,
    pub state: String,
    pub binding_generation: u64,
    pub root_capture_epoch: Option<String>,
    pub root_actor_epoch: Option<String>,
    pub desired_materialized_name: String,
    pub desired_materialization_fingerprint: String,
    pub applied_materialized_name: Option<String>,
    pub applied_materialization_fingerprint: Option<String>,
    pub operation_revision: u64,
    pub last_error_code: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootOperationRecord {
    pub operation_uuid: String,
    pub library_uuid: String,
    pub root_uuid: String,
    pub kind: String,
    pub state: String,
    pub phase: String,
    pub request_hash: String,
    pub request_json: String,
    pub expected_operation_revision: u64,
    pub expected_binding_generation: Option<u64>,
    pub operation_revision: u64,
    pub cutover_committed: bool,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootRelocationRecord {
    pub operation: RootOperationRecord,
    pub mode: String,
    pub desired_materialization_fingerprint: String,
    pub source_path: String,
    pub source_canonical_key: String,
    pub source_filesystem_identity: Option<String>,
    pub target_path: String,
    pub target_canonical_key: String,
    pub target_filesystem_identity: Option<String>,
    pub staging_path: Option<String>,
    pub transferred_bytes: u64,
    pub verified_bytes: u64,
    pub verification_digest: Option<String>,
    pub cleanup_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFolderSettingsRecord {
    pub library_uuid: String,
    pub library_root_uuid: String,
    pub default_placement_base: String,
    pub operation_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncRootRecord {
    pub library_uuid: String,
    pub library_root_uuid: String,
    pub root_uuid: String,
    pub root_folder_uuid: String,
    pub requested_name: String,
    pub materialized_name: String,
    pub materialization_fingerprint: String,
    pub logical_revision: u64,
    pub lifecycle: String,
    pub inventory_state: String,
    pub logical_bytes: Option<u64>,
    pub checkpoint_uuid: Option<String>,
    pub inventory_revision: u64,
    pub operation_revision: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootRebindEvidenceRecord {
    pub marker_identity: String,
    pub prior_cutover_snapshot_uuid: String,
}

#[derive(Debug, Clone)]
pub struct NewLocalRootBinding {
    pub library_uuid: String,
    pub root_uuid: String,
    pub local_binding_uuid: String,
    pub local_path: String,
    pub canonical_path_key: String,
    pub placement_parent: String,
    pub placement_parent_key: String,
    pub marker_identity: String,
    pub filesystem_identity: Option<String>,
    pub state: String,
    pub binding_generation: u64,
    pub root_capture_epoch: Option<String>,
    pub root_actor_epoch: Option<String>,
    pub desired_materialized_name: String,
    pub desired_materialization_fingerprint: String,
    pub applied_materialized_name: Option<String>,
    pub applied_materialization_fingerprint: Option<String>,
    pub operation_revision: u64,
    pub verification_json: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct PlannedRootOperation {
    pub operation_uuid: String,
    pub root_uuid: String,
    pub kind: String,
    pub phase: String,
    pub request_hash: String,
    pub request: serde_json::Value,
    pub expected_operation_revision: u64,
    pub expected_binding_generation: Option<u64>,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootComparisonPlanRecord {
    pub plan_uuid: String,
    pub operation_uuid: String,
    pub root_uuid: String,
    pub base_digest: String,
    pub local_digest: String,
    pub remote_digest: String,
    pub base_frontier: serde_json::Value,
    pub local_frontier: serde_json::Value,
    pub remote_frontier: serde_json::Value,
    pub root_revision: u64,
    pub account_generation: u64,
    pub provider_generation: u64,
    pub status: String,
    pub item_count: u64,
    pub conflict_count: u64,
    pub chosen_mode: Option<String>,
    pub expires_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootStopPlanRecord {
    pub plan_uuid: String,
    pub operation_uuid: String,
    pub root_uuid: String,
    pub local_binding_uuid: String,
    pub binding_generation: u64,
    pub root_capture_epoch: String,
    pub root_actor_epoch: String,
    pub pending_work: u64,
    pub published_work: u64,
    pub checkpoint_uuid: Option<String>,
    pub event_frontier: serde_json::Value,
    pub catalog_snapshot_digest: String,
    pub provider_receipts: serde_json::Value,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct RootReconciliationWorkRecord {
    pub plan_uuid: String,
    pub item_ordinal: u64,
    pub entity_uuid: String,
    pub field_name: String,
    pub resolution: String,
    pub selected_value: Option<String>,
    pub selected_entity: Option<String>,
    pub local_binding_uuid: String,
    pub binding_generation: u64,
}

#[derive(Debug, Clone)]
pub struct RootVisibilityReconciliationRecord {
    pub snapshot_uuid: String,
    pub operation_uuid: String,
    pub provider_id: String,
    pub account_uuid: String,
    pub remote_namespace: String,
    pub provider_binding_epoch: String,
    pub provider_transport_generation: u64,
    pub receipts: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct RootPublishAsNewRecord {
    pub operation_uuid: String,
    pub enrollment_operation_uuid: String,
    pub library_uuid: String,
    pub source_root_uuid: String,
    pub new_root_uuid: String,
    pub new_root_folder_uuid: String,
    pub new_local_binding_uuid: String,
    pub root_capture_epoch: String,
    pub root_actor_epoch: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootComparisonItemRecord {
    pub item_ordinal: u64,
    pub entity_uuid: String,
    pub field_name: String,
    pub classification: String,
    pub suggested_action: Option<String>,
    pub resolution: Option<String>,
    pub conflict_uuid: Option<String>,
    pub redacted_detail: Option<serde_json::Value>,
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
    pub fn replay_restore_operation(
        &self,
        token: &str,
        operation_uuid: &str,
        request: &serde_json::Value,
    ) -> anyhow::Result<Option<Vec<SyncRestorePlan>>> {
        let request_json = serde_json::to_string(request)?;
        let request_hash = crate::sync_folders::request_hash(request)?;
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let Some((existing_json, kind)) = connection
            .query_row(
                "SELECT request_json,kind FROM sync_orchestration_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(None);
        };
        if kind != "restore" || existing_json != request_json {
            bail!("idempotency-key-reused");
        }
        let claimed: Option<(String, String)> = connection
            .query_row(
                "SELECT COALESCE(operation_uuid,''),COALESCE(request_hash,'') FROM sync_restore_preflights WHERE token=?1",
                [token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if claimed
            .as_ref()
            .is_none_or(|value| value.0 != operation_uuid || value.1 != request_hash)
        {
            bail!("restore-preflight-expired-or-changed");
        }
        let plans = restore_plans_for_operation_in_transaction(&connection, operation_uuid)?;
        if plans.is_empty() {
            bail!("restore-operation-exists-without-plans");
        }
        Ok(Some(plans))
    }

    pub fn claim_restore_and_record_plans(
        &self,
        token: &str,
        operation_uuid: &str,
        request: &serde_json::Value,
        plans: &[RestorePlanInput],
    ) -> anyhow::Result<(bool, Vec<SyncRestorePlan>)> {
        let now = unix_timestamp_millis();
        let request_json = serde_json::to_string(request)?;
        let request_hash = crate::sync_folders::request_hash(request)?;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some((existing_json, kind)) = transaction
            .query_row(
                "SELECT request_json,kind FROM sync_orchestration_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if kind != "restore" || existing_json != request_json {
                bail!("idempotency-key-reused");
            }
            let claimed: Option<(String, String)> = transaction
                .query_row(
                    "SELECT COALESCE(operation_uuid,''),COALESCE(request_hash,'') FROM sync_restore_preflights WHERE token=?1",
                    [token],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if claimed
                .as_ref()
                .is_none_or(|value| value.0 != operation_uuid || value.1 != request_hash)
            {
                bail!("restore-preflight-expired-or-changed");
            }
            let existing = restore_plans_for_operation_in_transaction(&transaction, operation_uuid)?;
            if existing.is_empty() {
                bail!("restore-operation-exists-without-plans");
            }
            transaction.commit()?;
            return Ok((false, existing));
        }
        let changed = transaction.execute(
            "UPDATE sync_restore_preflights SET state='claimed',operation_uuid=?1,request_hash=?2,consumed_at=?3 WHERE token=?4 AND request_json=?5 AND request_hash=?2 AND state='ready' AND expires_at>=?3",
            params![operation_uuid,request_hash,now,token,request_json],
        )?;
        if changed != 1 {
            bail!("restore-preflight-expired-or-changed");
        }
        transaction.execute(
            "INSERT INTO sync_orchestration_operations(operation_uuid,library_uuid,kind,state,phase,request_json,created_at,updated_at) VALUES (?1,NULL,'restore','running','operation-and-plans-recorded',?2,?3,?3)",
            params![operation_uuid,request_json,now],
        )?;
        let mut recorded = Vec::with_capacity(plans.len());
        for plan in plans {
            let remote: Option<(Vec<u8>, String)> = transaction
                .query_row(
                    "SELECT descriptor_sha256,name FROM provider_remote_libraries WHERE account_uuid=?1 AND library_uuid=?2 AND compatible=1",
                    params![plan.account_uuid, plan.library_uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (descriptor_sha256, name) =
                remote.ok_or_else(|| anyhow::anyhow!("remote-library-not-restorable"))?;
            let restore_uuid = uuid::Uuid::now_v7().to_string();
            transaction.execute(
                "INSERT INTO sync_restore_plans(restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,descriptor_sha256,state,phase,restore_mode,inventory_fence,request_hash,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'restoring','operation-and-plans-recorded',?8,?9,?10,?11,?11)",
                params![restore_uuid,operation_uuid,plan.account_uuid,plan.library_uuid,name,plan.managed_root.to_string_lossy(),descriptor_sha256,plan.restore_mode,plan.inventory_fence,request_hash,now],
            )?;
            for root in &plan.roots {
                let binding = root.selected_binding.as_ref();
                transaction.execute(
                    "INSERT INTO sync_restore_root_plans(restore_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialization_fingerprint,logical_revision,selected,target_path,canonical_target_key,local_binding_uuid,binding_generation,logical_bytes,phase,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'planned',?14,?14)",
                    params![restore_uuid,root.root_uuid,root.root_folder_uuid,root.requested_name,root.materialized_name,root.materialization_fingerprint,i64::try_from(root.logical_revision)?,i64::from(binding.is_some()),binding.map(|value|value.local_path.as_str()),binding.map(|value|value.canonical_path_key.as_str()),binding.map(|value|value.local_binding_uuid.as_str()),binding.map(|value|i64::try_from(value.binding_generation)).transpose()?,root.logical_bytes.map(i64::try_from).transpose()?,now],
                )?;
            }
            recorded.push(SyncRestorePlan {
                restore_uuid,
                operation_uuid: operation_uuid.to_owned(),
                account_uuid: plan.account_uuid.clone(),
                library_uuid: plan.library_uuid.clone(),
                name,
                managed_root: plan.managed_root.to_string_lossy().into_owned(),
                state: "restoring".into(),
                phase: "operation-and-plans-recorded".into(),
                catalog_ready: false,
                media_items_done: 0,
                media_items_total: None,
                media_bytes_done: 0,
                media_bytes_total: None,
                error_code: None,
                updated_at: now,
            });
        }
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok((true, recorded))
    }

    pub fn import_restore_root_inventory(
        &self,
        restore_uuid: &str,
        library_uuid: &str,
        roots: &[RestoreRootPlanInput],
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        for root in roots {
            transaction.execute(
                "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,logical_revision,lifecycle,inventory_state,logical_bytes,inventory_revision,operation_revision,updated_at) VALUES (?1,?2,?3,?4,?5,lower(?5),?6,?7,'active',?8,?9,1,0,?10)",
                params![library_uuid,root.root_uuid,root.root_folder_uuid,root.requested_name,root.materialized_name,root.materialization_fingerprint,i64::try_from(root.logical_revision)?,"unselected",root.logical_bytes.map(i64::try_from).transpose()?,now],
            )?;
            transaction.execute(
                "UPDATE sync_restore_root_plans SET phase='inventory-imported',updated_at=?1 WHERE restore_uuid=?2 AND root_uuid=?3",
                params![now, restore_uuid, root.root_uuid],
            )?;
        }
        transaction.execute(
            "UPDATE sync_restore_plans SET phase='inventory-imported',updated_at=?2 WHERE restore_uuid=?1",
            params![restore_uuid, now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn stage_restore_root_bindings(
        &self,
        restore_uuid: &str,
        library_uuid: &str,
        roots: &[RestoreRootPlanInput],
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        for root in roots {
            if let Some(binding) = &root.selected_binding {
                transaction.execute(
                    "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,marker_identity,filesystem_identity,state,binding_generation,root_capture_epoch,root_actor_epoch,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,operation_revision,verification_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'materializing',1,?10,?11,?12,lower(?12),?13,0,?14,?15,?15)",
                    params![library_uuid,root.root_uuid,binding.local_binding_uuid,binding.local_path,binding.canonical_path_key,binding.placement_parent,binding.placement_parent_key,binding.marker_identity,binding.filesystem_identity,binding.root_capture_epoch,binding.root_actor_epoch,binding.desired_materialized_name,binding.desired_materialization_fingerprint,serde_json::to_string(&binding.verification_json)?,now],
                )?;
                transaction.execute(
                    "UPDATE sync_restore_root_plans SET phase='binding-staged',updated_at=?1 WHERE restore_uuid=?2 AND root_uuid=?3",
                    params![now, restore_uuid, root.root_uuid],
                )?;
            }
        }
        transaction.execute(
            "UPDATE sync_restore_plans SET phase='host-bindings-staged',updated_at=?2 WHERE restore_uuid=?1",
            params![restore_uuid, now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Root-aware Catalog execution and the Host operation journal are now
    /// installed. UI and relocation are independent consumers of this gate.
    pub fn sync_folder_v2_enabled(&self) -> bool {
        true
    }

    pub fn sync_folder_settings(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<SyncFolderSettingsRecord> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let (library_root_uuid, managed_root): (String, String) = transaction
            .query_row(
                "SELECT root_uuid,managed_root FROM libraries WHERE library_uuid=?1",
                [library_uuid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("library-not-found")?;
        let now = unix_timestamp_millis();
        let canonical = crate::sync_folders::canonical_path_key(Path::new(&managed_root));
        transaction.execute(
            "INSERT OR IGNORE INTO library_local_settings(library_uuid,library_root_uuid,default_placement_base,canonical_placement_key,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?5)",
            params![library_uuid,library_root_uuid,managed_root,canonical,now],
        )?;
        let record = transaction.query_row(
            "SELECT library_uuid,library_root_uuid,default_placement_base,operation_revision FROM library_local_settings WHERE library_uuid=?1",
            [library_uuid],
            |row| Ok(SyncFolderSettingsRecord {
                library_uuid: row.get(0)?,
                library_root_uuid: row.get(1)?,
                default_placement_base: row.get(2)?,
                operation_revision: row.get::<_, i64>(3)?.max(0) as u64,
            }),
        )?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn update_sync_folder_settings(
        &self,
        library_uuid: &str,
        expected_revision: u64,
        default_placement_base: &Path,
    ) -> anyhow::Result<SyncFolderSettingsRecord> {
        let current = self.sync_folder_settings(library_uuid)?;
        if current.operation_revision != expected_revision {
            bail!("stale-operation-revision");
        }
        let normalized = crate::library_path::normalize_absolute(default_placement_base)
            .map_err(|_| anyhow::anyhow!("invalid-path"))?;
        let normalized_text = normalized.to_string_lossy().into_owned();
        let key = crate::sync_folders::canonical_path_key(&normalized);
        let now = unix_timestamp_millis();
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let changed = connection.execute(
            "UPDATE library_local_settings SET default_placement_base=?1,canonical_placement_key=?2,operation_revision=operation_revision+1,updated_at=?3 WHERE library_uuid=?4 AND operation_revision=?5",
            params![normalized_text,key,now,library_uuid,i64::try_from(expected_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        drop(connection);
        self.sync_folder_settings(library_uuid)
    }

    pub fn upsert_remote_sync_root(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        root_folder_uuid: &str,
        requested_name: &str,
        materialized_name: &str,
        materialization_fingerprint: &str,
        logical_revision: u64,
        lifecycle: &str,
        logical_bytes: Option<u64>,
        checkpoint_uuid: Option<&str>,
    ) -> anyhow::Result<bool> {
        let settings = self.sync_folder_settings(library_uuid)?;
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let existing: Option<(i64, String, String)> = transaction.query_row(
            "SELECT logical_revision,lifecycle,inventory_state FROM remote_sync_roots WHERE library_uuid=?1 AND root_uuid=?2",
            params![library_uuid,root_uuid],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).optional()?;
        if existing
            .as_ref()
            .is_some_and(|(revision, _, _)| *revision > logical_revision as i64)
        {
            bail!("stale-remote-root-revision");
        }
        let has_binding: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2)",
            params![library_uuid,root_uuid],
            |row| row.get(0),
        )?;
        let inventory_state = if lifecycle == "tombstoned" {
            "tombstoned"
        } else if has_binding {
            existing
                .as_ref()
                .map(|value| value.2.as_str())
                .unwrap_or("unselected")
        } else {
            "placement-required"
        };
        let actionable = !has_binding
            && lifecycle == "active"
            && existing.as_ref().is_none_or(|(revision, _, state)| {
                *revision != logical_revision as i64 || state != "placement-required"
            });
        transaction.execute(
            "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,logical_revision,lifecycle,inventory_state,logical_bytes,checkpoint_uuid,inventory_revision,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,1,?13) ON CONFLICT(library_uuid,root_uuid) DO UPDATE SET requested_name=excluded.requested_name,materialized_name=excluded.materialized_name,materialized_name_key=excluded.materialized_name_key,materialization_fingerprint=excluded.materialization_fingerprint,logical_revision=excluded.logical_revision,lifecycle=excluded.lifecycle,inventory_state=excluded.inventory_state,logical_bytes=excluded.logical_bytes,checkpoint_uuid=excluded.checkpoint_uuid,inventory_revision=CASE WHEN remote_sync_roots.logical_revision<>excluded.logical_revision OR remote_sync_roots.lifecycle<>excluded.lifecycle THEN remote_sync_roots.inventory_revision+1 ELSE remote_sync_roots.inventory_revision END,operation_revision=CASE WHEN remote_sync_roots.logical_revision<>excluded.logical_revision OR remote_sync_roots.lifecycle<>excluded.lifecycle THEN remote_sync_roots.operation_revision+1 ELSE remote_sync_roots.operation_revision END,updated_at=excluded.updated_at WHERE remote_sync_roots.root_folder_uuid=excluded.root_folder_uuid",
            params![library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name.to_lowercase(),materialization_fingerprint,i64::try_from(logical_revision)?,lifecycle,inventory_state,logical_bytes.map(i64::try_from).transpose()?,checkpoint_uuid,now],
        )?;
        transaction.execute(
            "UPDATE local_root_bindings SET desired_materialized_name=?1,desired_materialized_name_key=?2,desired_materialization_fingerprint=?3,updated_at=?4 WHERE library_uuid=?5 AND root_uuid=?6 AND (desired_materialized_name<>?1 OR desired_materialization_fingerprint<>?3)",
            params![materialized_name,materialized_name.to_lowercase(),materialization_fingerprint,now,library_uuid,root_uuid],
        )?;
        let _ = settings;
        transaction.commit()?;
        Ok(actionable)
    }

    pub fn pending_automatic_root_renames(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Vec<(LocalRootBindingRecord, RemoteSyncRootRecord)>> {
        let bindings = self.local_root_bindings(library_uuid)?;
        let mut pending = Vec::new();
        for binding in bindings {
            if !matches!(
                binding.state.as_str(),
                "active" | "paused" | "local-rename-required"
            ) || (binding.state != "local-rename-required"
                && binding.applied_materialization_fingerprint.as_deref()
                    == Some(binding.desired_materialization_fingerprint.as_str()))
            {
                continue;
            }
            if let Some(descriptor) = self.remote_sync_root(library_uuid, &binding.root_uuid)? {
                pending.push((binding, descriptor));
            }
        }
        Ok(pending)
    }

    /// A canonical revision may change the fingerprint without moving the root.
    /// Confirm that fact under physical identity and Host revision fences.
    pub(crate) fn reconcile_same_name_root_materialization(
        &self,
        binding: &LocalRootBindingRecord,
        descriptor: &RemoteSyncRootRecord,
    ) -> anyhow::Result<bool> {
        use crate::sync_folders::{canonical_path_key, SyncRootMarker, VerifiedRootMarker};
        let source = Path::new(&binding.local_path);
        let target = Path::new(&binding.placement_parent).join(&descriptor.materialized_name);
        if source.file_name() != Some(std::ffi::OsStr::new(&descriptor.materialized_name))
            || canonical_path_key(source) != canonical_path_key(&target)
        {
            return Ok(false);
        }
        if binding.library_uuid != descriptor.library_uuid
            || binding.root_uuid != descriptor.root_uuid
            || binding.desired_materialized_name != descriptor.materialized_name
            || binding.desired_materialization_fingerprint != descriptor.materialization_fingerprint
            || binding.canonical_path_key != canonical_path_key(source)
            || !matches!(
                binding.state.as_str(),
                "active" | "paused" | "local-rename-required"
            )
            || descriptor.lifecycle != "active"
        {
            bail!("stale-root-materialization");
        }
        let marker = SyncRootMarker {
            format_version: 1,
            library_uuid: binding.library_uuid.clone(),
            root_uuid: binding.root_uuid.clone(),
            root_folder_uuid: descriptor.root_folder_uuid.clone(),
            local_binding_uuid: binding.local_binding_uuid.clone(),
            binding_generation: binding.binding_generation,
        };
        let mut evidence = VerifiedRootMarker::open(source, &marker)?;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let (current, marker_identity, filesystem_identity) = transaction.query_row(
            "SELECT library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,binding_generation,root_capture_epoch,root_actor_epoch,desired_materialized_name,desired_materialization_fingerprint,applied_materialized_name,applied_materialization_fingerprint,operation_revision,last_error_code,marker_identity,filesystem_identity FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
            params![binding.library_uuid,binding.root_uuid],
            |row| Ok((local_root_binding_record_from_row(row)?, row.get::<_, Option<String>>(17)?, row.get::<_, Option<String>>(18)?)),
        )?;
        if current != *binding {
            bail!("stale-root-materialization");
        }
        if marker_identity.as_deref() != Some(evidence.marker_identity())
            || filesystem_identity.as_deref() != Some(evidence.directory_identity())
        {
            bail!("path-changed");
        }
        let descriptor_current: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_sync_roots WHERE library_uuid=?1 AND root_uuid=?2 AND root_folder_uuid=?3 AND materialized_name=?4 AND materialization_fingerprint=?5 AND logical_revision=?6 AND lifecycle='active')",
            params![descriptor.library_uuid,descriptor.root_uuid,descriptor.root_folder_uuid,descriptor.materialized_name,descriptor.materialization_fingerprint,i64::try_from(descriptor.logical_revision)?],
            |row| row.get(0),
        )?;
        if !descriptor_current {
            bail!("stale-root-materialization");
        }
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND state IN ('pending','running','waiting'))",
            params![binding.library_uuid,binding.root_uuid],
            |row| row.get::<_, bool>(0),
        )? {
            bail!("operation-in-progress");
        }
        let resume_state = if binding.state == "local-rename-required" {
            // The first failure in this generation retains the original pause
            // intent even if later fingerprint revisions caused another failure.
            transaction.query_row(
                "SELECT json_extract(operation.request_json,'$.resumeState') FROM root_operations operation JOIN root_relocation_operations relocation USING(operation_uuid) WHERE operation.library_uuid=?1 AND operation.root_uuid=?2 AND operation.kind='automatic-rename' AND operation.state='failed' AND operation.phase='local-rename-required' AND operation.cutover_committed=0 AND operation.expected_binding_generation=?3 AND json_extract(operation.request_json,'$.localBindingUuid')=?4 AND relocation.source_canonical_key=?5 AND json_extract(operation.request_json,'$.resumeState') IN ('active','paused') ORDER BY operation.expected_operation_revision,operation.created_at,operation.operation_uuid LIMIT 1",
                params![binding.library_uuid,binding.root_uuid,i64::try_from(binding.binding_generation)?,binding.local_binding_uuid,binding.canonical_path_key],
                |row| row.get::<_, String>(0),
            ).optional()?.context("invalid-root-state")?
        } else {
            binding.state.clone()
        };
        evidence.verify_unchanged()?;
        transaction.execute(
            "UPDATE local_root_bindings SET applied_materialized_name=desired_materialized_name,applied_materialized_name_key=desired_materialized_name_key,applied_materialization_fingerprint=desired_materialization_fingerprint,state=?1,last_error_code=NULL,last_error_detail=NULL,operation_revision=operation_revision+1,updated_at=?2 WHERE library_uuid=?3 AND root_uuid=?4 AND (applied_materialized_name IS NOT desired_materialized_name OR applied_materialization_fingerprint IS NOT desired_materialization_fingerprint OR state<>?1 OR last_error_code IS NOT NULL)",
            params![resume_state,unix_timestamp_millis(),binding.library_uuid,binding.root_uuid],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn remote_sync_roots(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Vec<RemoteSyncRootRecord>> {
        let settings = self.sync_folder_settings(library_uuid)?;
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialization_fingerprint,logical_revision,lifecycle,inventory_state,logical_bytes,checkpoint_uuid,inventory_revision,operation_revision FROM remote_sync_roots WHERE library_uuid=?1 ORDER BY materialized_name_key,root_uuid",
        )?;
        let rows = statement.query_map([library_uuid], |row| {
            Ok(RemoteSyncRootRecord {
                library_uuid: row.get(0)?,
                library_root_uuid: settings.library_root_uuid.clone(),
                root_uuid: row.get(1)?,
                root_folder_uuid: row.get(2)?,
                requested_name: row.get(3)?,
                materialized_name: row.get(4)?,
                materialization_fingerprint: row.get(5)?,
                logical_revision: row.get::<_, i64>(6)?.max(0) as u64,
                lifecycle: row.get(7)?,
                inventory_state: row.get(8)?,
                logical_bytes: row.get::<_, Option<i64>>(9)?.map(|v| v.max(0) as u64),
                checkpoint_uuid: row.get(10)?,
                inventory_revision: row.get::<_, i64>(11)?.max(0) as u64,
                operation_revision: row.get::<_, i64>(12)?.max(0) as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn remote_sync_root(
        &self,
        library_uuid: &str,
        root_uuid: &str,
    ) -> anyhow::Result<Option<RemoteSyncRootRecord>> {
        Ok(self
            .remote_sync_roots(library_uuid)?
            .into_iter()
            .find(|root| root.root_uuid == root_uuid))
    }

    pub fn root_operation(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<Option<RootOperationRecord>> {
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).query_row(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],root_operation_record_from_row,
        ).optional().map_err(Into::into)
    }

    pub fn root_operations(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        cursor: Option<&str>,
        limit: usize,
        state_filter: &str,
    ) -> anyhow::Result<Vec<RootOperationRecord>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND (?3='all' OR (?3='active' AND state IN ('pending','running','waiting')) OR (?3='terminal' AND state IN ('completed','cancelled','failed'))) AND (?4 IS NULL OR (updated_at,operation_uuid)<(SELECT updated_at,operation_uuid FROM root_operations WHERE operation_uuid=?4 AND library_uuid=?1 AND root_uuid=?2)) ORDER BY updated_at DESC,operation_uuid DESC LIMIT ?5",
        )?;
        let rows = statement.query_map(
            params![
                library_uuid,
                root_uuid,
                state_filter,
                cursor,
                i64::try_from(limit)?
            ],
            root_operation_record_from_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn latest_root_operation_of_kind(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        kind: &str,
    ) -> anyhow::Result<Option<RootOperationRecord>> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND kind=?3 ORDER BY updated_at DESC,operation_uuid DESC LIMIT 1",
                params![library_uuid,root_uuid,kind],
                root_operation_record_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn root_rebind_evidence(
        &self,
        library_uuid: &str,
        root_uuid: &str,
    ) -> anyhow::Result<Option<RootRebindEvidenceRecord>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection
            .query_row(
                "SELECT marker_identity FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2 AND state='unavailable' AND marker_identity IS NOT NULL AND NOT EXISTS (SELECT 1 FROM root_cutover_snapshots snapshot WHERE snapshot.library_uuid=?1 AND snapshot.root_uuid=?2)",
                params![library_uuid, root_uuid],
                |row| Ok(RootRebindEvidenceRecord {
                    marker_identity: row.get(0)?,
                    prior_cutover_snapshot_uuid: uuid::Uuid::nil().to_string(),
                }),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn library_sync_last_success_at(&self, library_uuid: &str) -> anyhow::Result<Option<i64>> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT last_success_at FROM library_sync_bindings WHERE library_uuid=?1",
                [library_uuid],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(Into::into)
    }

    pub fn root_relocation_operation(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<Option<RootRelocationRecord>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection
            .query_row(
                "SELECT operation.operation_uuid,operation.library_uuid,operation.root_uuid,operation.kind,operation.state,operation.phase,operation.request_hash,operation.request_json,operation.expected_operation_revision,operation.expected_binding_generation,operation.operation_revision,operation.cutover_committed,operation.error_code,operation.created_at,operation.updated_at,relocation.mode,relocation.desired_materialization_fingerprint,relocation.source_path,relocation.source_canonical_key,relocation.source_filesystem_identity,relocation.target_path,relocation.target_canonical_key,relocation.target_filesystem_identity,relocation.staging_path,relocation.transferred_bytes,relocation.verified_bytes,relocation.verification_digest,relocation.cleanup_state FROM root_operations operation JOIN root_relocation_operations relocation ON relocation.operation_uuid=operation.operation_uuid WHERE operation.operation_uuid=?1",
                [operation_uuid],
                |row| {
                    let expected_binding_generation: Option<i64> = row.get(9)?;
                    let operation_revision: i64 = row.get(10)?;
                    let transferred_bytes: i64 = row.get(24)?;
                    let verified_bytes: i64 = row.get(25)?;
                    Ok(RootRelocationRecord {
                        operation: RootOperationRecord {
                            operation_uuid: row.get(0)?,
                            library_uuid: row.get(1)?,
                            root_uuid: row.get(2)?,
                            kind: row.get(3)?,
                            state: row.get(4)?,
                            phase: row.get(5)?,
                            request_hash: row.get(6)?,
                            request_json: row.get(7)?,
                            expected_operation_revision: row.get::<_, i64>(8)?.max(0) as u64,
                            expected_binding_generation: expected_binding_generation.map(|value| value.max(0) as u64),
                            operation_revision: operation_revision.max(0) as u64,
                            cutover_committed: row.get::<_, i64>(11)? != 0,
                            error_code: row.get(12)?,
                            created_at: row.get(13)?,
                            updated_at: row.get(14)?,
                        },
                        mode: row.get(15)?,
                        desired_materialization_fingerprint: row.get(16)?,
                        source_path: row.get(17)?,
                        source_canonical_key: row.get(18)?,
                        source_filesystem_identity: row.get(19)?,
                        target_path: row.get(20)?,
                        target_canonical_key: row.get(21)?,
                        target_filesystem_identity: row.get(22)?,
                        staging_path: row.get(23)?,
                        transferred_bytes: transferred_bytes.max(0) as u64,
                        verified_bytes: verified_bytes.max(0) as u64,
                        verification_digest: row.get(26)?,
                        cleanup_state: row.get(27)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn pending_root_relocations(&self) -> anyhow::Result<Vec<RootRelocationRecord>> {
        let operation_uuids = {
            let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
            let mut statement = connection.prepare(
            "SELECT operation.operation_uuid FROM root_operations operation JOIN root_relocation_operations relocation ON relocation.operation_uuid=operation.operation_uuid WHERE operation.kind IN ('relocate','automatic-rename') AND (operation.state IN ('pending','running','waiting') OR (operation.state IN ('cancelled','failed') AND relocation.cleanup_state NOT IN ('not-required','failed'))) ORDER BY operation.created_at,operation.operation_uuid",
            )?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        operation_uuids
            .into_iter()
            .map(|operation_uuid| {
                self.root_relocation_operation(&operation_uuid)?
                    .context("root-relocation-not-found")
            })
            .collect()
    }

    pub fn pending_reenable_plan_uuid(
        &self,
        library_uuid: &str,
        root_uuid: &str,
    ) -> anyhow::Result<Option<String>> {
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row("SELECT plan_uuid FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND kind='re-enable' AND state='running' AND phase='reconciling' ORDER BY updated_at DESC LIMIT 1",params![library_uuid,root_uuid],|row|row.get(0)).optional().map_err(Into::into)
    }

    pub fn pending_root_reconciliations(&self) -> anyhow::Result<Vec<(String, String)>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement=connection.prepare("SELECT library_uuid,root_uuid FROM local_root_bindings WHERE state='reconcile-required' ORDER BY updated_at,library_uuid,root_uuid")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn root_has_cutover_lineage(
        &self,
        library_uuid: &str,
        root_uuid: &str,
    ) -> anyhow::Result<bool> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM root_cutover_snapshots WHERE library_uuid=?1 AND root_uuid=?2)",
                params![library_uuid, root_uuid],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn set_root_operation_plan(
        &self,
        operation_uuid: &str,
        plan_uuid: &str,
        result: &serde_json::Value,
        phase: &str,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let result_json = serde_json::to_string(result)?;
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let changed = connection.execute(
            "UPDATE root_operations SET state='waiting',phase=?1,result_json=?2,plan_uuid=?3,operation_revision=operation_revision+1,updated_at=?4 WHERE operation_uuid=?5 AND state IN ('pending','running','waiting') AND (plan_uuid IS NULL OR plan_uuid=?3)",
            params![phase,result_json,plan_uuid,now,operation_uuid],
        )?;
        drop(connection);
        if changed == 0 && self.root_operation(operation_uuid)?.is_none() {
            bail!("root-operation-not-found");
        }
        self.root_operation(operation_uuid)?
            .context("root-operation-not-found")
    }

    pub fn begin_root_operations_aggregate(
        &self,
        library_uuid: &str,
        plan_uuid: &str,
        operations: &[PlannedRootOperation],
    ) -> anyhow::Result<Vec<RootOperationRecord>> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        for operation in operations {
            let request_json = serde_json::to_string(&operation.request)?;
            let result_json = serde_json::to_string(&operation.result)?;
            if let Some(existing) = transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [&operation.operation_uuid],root_operation_record_from_row,
            ).optional()? {
                if existing.library_uuid != library_uuid
                    || existing.root_uuid != operation.root_uuid
                    || existing.kind != operation.kind
                    || existing.request_hash != operation.request_hash
                    || existing.request_json != request_json
                {
                    bail!("idempotency-key-reused");
                }
                continue;
            }
            let state: (i64,Option<String>,Option<i64>,Option<i64>) = transaction.query_row(
                "SELECT remote.operation_revision,binding.state,binding.binding_generation,binding.operation_revision FROM remote_sync_roots remote LEFT JOIN local_root_bindings binding ON binding.library_uuid=remote.library_uuid AND binding.root_uuid=remote.root_uuid WHERE remote.library_uuid=?1 AND remote.root_uuid=?2",
                params![library_uuid,operation.root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).optional()?.context("root-not-found")?;
            if state.3.unwrap_or(state.0) != i64::try_from(operation.expected_operation_revision)? {
                bail!("stale-operation-revision");
            }
            let sealed_lineage: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM root_cutover_snapshots WHERE library_uuid=?1 AND root_uuid=?2)",
                params![library_uuid,operation.root_uuid],
                |row| row.get(0),
            )?;
            if matches!(operation.kind.as_str(), "select" | "rebind")
                && (state.1.as_deref() == Some("local-only-fork") || sealed_lineage)
            {
                bail!("comparison-required");
            }
            if state.2.map(|value| value as u64) != operation.expected_binding_generation {
                if state.2.is_some() || operation.expected_binding_generation.is_some() {
                    bail!("stale-binding-generation");
                }
            }
            let open: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND state IN ('pending','running','waiting'))",
                params![library_uuid,operation.root_uuid],|row|row.get(0),
            )?;
            if open {
                bail!("operation-in-progress");
            }
            transaction.execute(
                "INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,result_json,expected_operation_revision,expected_binding_generation,plan_uuid,created_at,updated_at) VALUES (?1,?2,?3,?4,'waiting',?5,?6,?7,?8,?9,?10,?11,?12,?12)",
                params![operation.operation_uuid,library_uuid,operation.root_uuid,operation.kind,operation.phase,operation.request_hash,request_json,result_json,i64::try_from(operation.expected_operation_revision)?,operation.expected_binding_generation.map(i64::try_from).transpose()?,plan_uuid,now],
            )?;
        }
        let mut output = Vec::new();
        for operation in operations {
            output.push(transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [&operation.operation_uuid],root_operation_record_from_row,
            )?);
        }
        transaction.commit()?;
        Ok(output)
    }

    pub fn complete_root_operations_aggregate(
        &self,
        library_uuid: &str,
        plan_uuid: &str,
        bindings: &[NewLocalRootBinding],
        deselections: &[(String, u64)],
    ) -> anyhow::Result<Vec<RootOperationRecord>> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        for binding in bindings {
            let operation: (String,String,String,String) = transaction.query_row(
                "SELECT operation_uuid,state,kind,COALESCE(plan_uuid,'') FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND plan_uuid=?3",
                params![library_uuid,binding.root_uuid,plan_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).context("stale-plan")?;
            if operation.2 != "select" || operation.3 != plan_uuid {
                bail!("stale-plan");
            }
            if operation.1 == "completed" {
                continue;
            }
            if operation.1 != "waiting" {
                bail!("operation-in-progress");
            }
            let previous: Option<(String,i64)> = transaction.query_row(
                "SELECT state,binding_generation FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
                params![library_uuid,binding.root_uuid],|row|Ok((row.get(0)?,row.get(1)?)),
            ).optional()?;
            let sealed_lineage: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM root_cutover_snapshots WHERE library_uuid=?1 AND root_uuid=?2)",
                params![library_uuid,binding.root_uuid],
                |row| row.get(0),
            )?;
            if previous
                .as_ref()
                .is_some_and(|(state, _)| state == "local-only-fork")
                || sealed_lineage
            {
                bail!("comparison-required");
            }
            if previous.is_some() {
                bail!("root-already-selected");
            }
        }
        for (root_uuid, expected_revision) in deselections {
            let (operation_revision, inventory_state, has_binding): (i64, String, bool) =
                transaction
                    .query_row(
                        "SELECT remote.operation_revision,remote.inventory_state,EXISTS(SELECT 1 FROM local_root_bindings binding WHERE binding.library_uuid=remote.library_uuid AND binding.root_uuid=remote.root_uuid) FROM remote_sync_roots remote WHERE remote.library_uuid=?1 AND remote.root_uuid=?2",
                        params![library_uuid, root_uuid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?
                    .context("root-not-found")?;
            let expected_revision = i64::try_from(*expected_revision)?;
            let already_acknowledged = operation_revision == expected_revision + 1
                && inventory_state == "unselected"
                && !has_binding;
            if has_binding || (operation_revision != expected_revision && !already_acknowledged) {
                bail!("stale-operation-revision");
            }
        }
        for binding in bindings {
            let operation_uuid: String = transaction.query_row(
                "SELECT operation_uuid FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND plan_uuid=?3",
                params![library_uuid,binding.root_uuid,plan_uuid],|row|row.get(0),
            )?;
            let verification_json = serde_json::to_string(&binding.verification_json)?;
            transaction.execute(
                "INSERT OR IGNORE INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,marker_identity,filesystem_identity,state,binding_generation,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,applied_materialized_name,applied_materialized_name_key,applied_materialization_fingerprint,operation_revision,verification_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,CASE WHEN ?15 IS NULL THEN NULL ELSE lower(?15) END,?16,?17,?18,?19,?19)",
                params![binding.library_uuid,binding.root_uuid,binding.local_binding_uuid,binding.local_path,binding.canonical_path_key,binding.placement_parent,binding.placement_parent_key,binding.marker_identity,binding.filesystem_identity,binding.state,i64::try_from(binding.binding_generation)?,binding.desired_materialized_name,binding.desired_materialized_name.to_lowercase(),binding.desired_materialization_fingerprint,binding.applied_materialized_name,binding.applied_materialization_fingerprint,i64::try_from(binding.operation_revision)?,verification_json,now],
            )?;
            transaction.execute("UPDATE root_operations SET state='completed',phase='completed',operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2",params![now,operation_uuid])?;
            transaction.execute("UPDATE remote_sync_roots SET inventory_state='unselected',operation_revision=operation_revision+1,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3",params![now,library_uuid,binding.root_uuid])?;
        }
        for (root_uuid, expected_revision) in deselections {
            transaction.execute(
                "UPDATE remote_sync_roots SET inventory_state='unselected',operation_revision=operation_revision+1,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3 AND operation_revision=?4 AND NOT EXISTS(SELECT 1 FROM local_root_bindings WHERE library_uuid=?2 AND root_uuid=?3)",
                params![now,library_uuid,root_uuid,i64::try_from(*expected_revision)?],
            )?;
        }
        let mut output = Vec::new();
        for binding in bindings {
            output.push(transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND plan_uuid=?3",
                params![library_uuid,binding.root_uuid,plan_uuid],root_operation_record_from_row,
            )?);
        }
        transaction.commit()?;
        Ok(output)
    }

    pub fn commit_stop_sync(
        &self,
        operation_uuid: &str,
        plan_uuid: &str,
        library_uuid: &str,
        root_uuid: &str,
        request_hash: &str,
        request: &serde_json::Value,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
        base_snapshot: &crate::sync_folders::RootSnapshot,
        current_work: &crate::sync_folders::RootStopWork,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some(existing)=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row).optional()? {
            if existing.library_uuid!=library_uuid || existing.root_uuid!=root_uuid || existing.kind!="stop-sync" || existing.request_hash!=request_hash { bail!("idempotency-key-reused"); }
            if existing.state=="completed" || existing.cutover_committed {
                transaction.commit()?;
                return Ok(existing);
            }
        }
        let plan:(String,i64,String,String,String,i64,i64,String,String,i64,String,Option<String>)=transaction.query_row("SELECT operation_uuid,expires_at,local_binding_uuid,root_capture_epoch,root_actor_epoch,pending_work,published_work,provider_binding_epoch,provider_receipts_json,provider_transport_generation,catalog_snapshot_digest,checkpoint_uuid FROM root_stop_plans WHERE plan_uuid=?1 AND library_uuid=?2 AND root_uuid=?3",params![plan_uuid,library_uuid,root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?,row.get(11)?))).context("stop-plan-not-found")?;
        if plan.0 != operation_uuid
            || plan.1 < now
            || plan.10 != base_snapshot.digest
            || plan.5 != i64::try_from(current_work.pending_work)?
            || plan.6 != i64::try_from(current_work.published_work)?
        {
            bail!("stale-plan");
        }
        let provider_binding_epoch = Uuid::parse_str(&plan.7)?;
        let planned_receipts = serde_json::from_str::<Vec<serde_json::Value>>(&plan.8)?;
        let mut current_keys = current_work.receipt_keys.clone();
        if let Some(checkpoint_uuid) = plan.11.as_deref() {
            current_keys.push(format!("checkpoints/{checkpoint_uuid}.checkpoint"));
        }
        current_keys.sort();
        current_keys.dedup();
        let mut planned_keys = planned_receipts
            .iter()
            .filter_map(|receipt| {
                receipt
                    .get("objectKey")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        planned_keys.sort();
        if planned_keys != current_keys {
            bail!("stale-plan");
        }
        for receipt in planned_receipts {
            let object_key = receipt
                .get("objectKey")
                .and_then(serde_json::Value::as_str)
                .context("stop-plan-receipt-corrupt")?;
            let expected_hash = receipt
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .context("stop-plan-receipt-corrupt")?;
            let unchanged: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_object_receipts WHERE library_uuid=?1 AND binding_epoch=?2 AND generation=?3 AND object_key=?4 AND lower(hex(sha256))=?5 AND state IN ('created','already-exists-matching'))",
                params![library_uuid,provider_binding_epoch.as_bytes().as_slice(),plan.9,object_key,expected_hash],
                |row| row.get(0),
            )?;
            if !unchanged {
                bail!("stale-plan");
            }
        }
        let binding:(String,i64,i64)=transaction.query_row("SELECT state,binding_generation,operation_revision FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",params![library_uuid,root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).context("local-root-binding-not-found")?;
        if binding.1 != i64::try_from(expected_binding_generation)? {
            bail!("stale-binding-generation");
        }
        if binding.2 != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        if binding.0 == "local-only-fork" {
            bail!("comparison-required");
        }
        if !matches!(
            binding.0.as_str(),
            "active" | "paused" | "reconcile-required"
        ) {
            bail!("invalid-root-state");
        }
        let _request_json = serde_json::to_string(request)?;
        let snapshot_uuid = Uuid::now_v7().to_string();
        let receipts = serde_json::json!({"baseSnapshot":base_snapshot,"sealedBindingGeneration":expected_binding_generation,"plannedReceipts":serde_json::from_str::<serde_json::Value>(&plan.8)?});
        transaction.execute("INSERT INTO root_cutover_snapshots(snapshot_uuid,operation_uuid,library_uuid,root_uuid,binding_generation,checkpoint_uuid,event_frontier_json,catalog_snapshot_digest,provider_receipts_json,remote_visibility,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'unknown',?10)",params![snapshot_uuid,operation_uuid,library_uuid,root_uuid,i64::try_from(expected_binding_generation)?,plan.11,serde_json::to_string(&base_snapshot.frontier)?,base_snapshot.digest,serde_json::to_string(&receipts)?,now])?;
        transaction.execute("INSERT INTO root_visibility_reconciliations(snapshot_uuid,operation_uuid,state,attempt_count,next_retry_at,updated_at) VALUES (?1,?2,'pending',0,?3,?3)",params![snapshot_uuid,operation_uuid,now])?;
        let fork_capture_epoch = Uuid::now_v7().to_string();
        let fork_actor_epoch = Uuid::now_v7().to_string();
        transaction.execute("UPDATE local_root_bindings SET state='local-only-fork',binding_generation=binding_generation+1,root_capture_epoch=?1,root_actor_epoch=?2,cutover_checkpoint_uuid=?3,cutover_frontier_json=?4,operation_revision=operation_revision+1,updated_at=?5 WHERE library_uuid=?6 AND root_uuid=?7 AND binding_generation=?8",params![fork_capture_epoch,fork_actor_epoch,plan.11,serde_json::to_string(&base_snapshot.frontier)?,now,library_uuid,root_uuid,i64::try_from(expected_binding_generation)?])?;
        transaction.execute("UPDATE root_operations SET state='running',phase='reconciling-visibility',cutover_committed=1,operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2",params![now,operation_uuid])?;
        transaction.execute("UPDATE remote_sync_roots SET operation_revision=operation_revision+1,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3",params![now,library_uuid,root_uuid])?;
        let result=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn pending_root_visibility_reconciliations(
        &self,
    ) -> anyhow::Result<Vec<RootVisibilityReconciliationRecord>> {
        let now = unix_timestamp_millis();
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement=connection.prepare("SELECT visibility.snapshot_uuid,visibility.operation_uuid,plan.provider_id,plan.account_uuid,plan.remote_namespace,plan.provider_binding_epoch,plan.provider_transport_generation,plan.provider_receipts_json FROM root_visibility_reconciliations visibility JOIN root_stop_plans plan ON plan.operation_uuid=visibility.operation_uuid WHERE visibility.state IN ('pending','retryable-unknown') AND COALESCE(visibility.next_retry_at,0)<=?1 ORDER BY visibility.updated_at,visibility.operation_uuid")?;
        let rows = statement.query_map([now], |row| {
            Ok(RootVisibilityReconciliationRecord {
                snapshot_uuid: row.get(0)?,
                operation_uuid: row.get(1)?,
                provider_id: row.get(2)?,
                account_uuid: row.get(3)?,
                remote_namespace: row.get(4)?,
                provider_binding_epoch: row.get(5)?,
                provider_transport_generation: row.get::<_, i64>(6)?.max(0) as u64,
                receipts: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn finish_root_visibility_reconciliation(
        &self,
        operation_uuid: &str,
        state: &str,
        evidence: &serde_json::Value,
    ) -> anyhow::Result<()> {
        if !matches!(state, "visible" | "hidden") {
            bail!("invalid-remote-visibility");
        }
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed=transaction.execute("UPDATE root_visibility_reconciliations SET state=?1,attempt_count=attempt_count+1,next_retry_at=NULL,last_error_code=NULL,checked_at=?2,updated_at=?2 WHERE operation_uuid=?3 AND state IN ('pending','retryable-unknown')",params![state,now,operation_uuid])?;
        if changed == 0 {
            let complete:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM root_visibility_reconciliations WHERE operation_uuid=?1 AND state IN ('visible','hidden'))",[operation_uuid],|row|row.get(0))?;
            if !complete {
                bail!("visibility-reconciliation-not-found");
            }
        }
        transaction.execute("UPDATE root_operations SET state='completed',phase='completed',result_json=?1,operation_revision=operation_revision+1,updated_at=?2 WHERE operation_uuid=?3 AND state='running'",params![serde_json::to_string(&serde_json::json!({"remoteVisibility":state,"evidence":evidence}))?,now,operation_uuid])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn retry_root_visibility_reconciliation(
        &self,
        operation_uuid: &str,
        error_code: &str,
        delay_ms: i64,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).execute("UPDATE root_visibility_reconciliations SET state='retryable-unknown',attempt_count=attempt_count+1,next_retry_at=?1,last_error_code=?2,checked_at=?3,updated_at=?3 WHERE operation_uuid=?4 AND state IN ('pending','retryable-unknown')",params![now.saturating_add(delay_ms.max(0)),error_code,now,operation_uuid])?;
        Ok(())
    }

    pub fn create_stop_sync_plan(
        &self,
        operation_uuid: &str,
        library_uuid: &str,
        root_uuid: &str,
        request_hash: &str,
        request: &serde_json::Value,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
        snapshot: &crate::sync_folders::RootSnapshot,
        work: &crate::sync_folders::RootStopWork,
    ) -> anyhow::Result<RootStopPlanRecord> {
        let now = unix_timestamp_millis();
        let expires_at = now + crate::sync_folders::PLAN_TTL_MILLIS;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some(plan_uuid) = transaction
            .query_row(
                "SELECT plan_uuid FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
        {
            transaction.commit()?;
            drop(connection);
            return self
                .stop_sync_plan(library_uuid, root_uuid, &plan_uuid)?
                .context("stop-plan-not-found");
        }
        let (state,binding_uuid,generation,revision,capture_epoch,actor_epoch):(String,String,i64,i64,String,String)=transaction.query_row("SELECT state,local_binding_uuid,binding_generation,operation_revision,COALESCE(root_capture_epoch,local_binding_uuid),COALESCE(root_actor_epoch,local_binding_uuid) FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",params![library_uuid,root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).context("local-root-binding-not-found")?;
        if generation != i64::try_from(expected_binding_generation)? {
            bail!("stale-binding-generation");
        }
        if revision != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        if !matches!(state.as_str(), "active" | "paused" | "reconcile-required") {
            bail!("invalid-root-state");
        }
        let (provider_id,account_uuid,binding_epoch,transport_generation,namespace,checkpoint):(String,String,Vec<u8>,i64,String,Option<Vec<u8>>)=transaction.query_row("SELECT plugin_id,account_uuid,binding_epoch,transport_generation,COALESCE(remote_namespace,library_uuid),latest_checkpoint_uuid FROM library_sync_bindings WHERE library_uuid=?1",[library_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).context("provider-binding-unavailable")?;
        let mut receipt_keys = work.receipt_keys.clone();
        if let Some(checkpoint) = checkpoint
            .as_ref()
            .map(|value| Uuid::from_slice(value))
            .transpose()?
        {
            receipt_keys.push(format!("checkpoints/{checkpoint}.checkpoint"));
        }
        receipt_keys.sort();
        receipt_keys.dedup();
        let mut receipts = Vec::new();
        for key in receipt_keys {
            if let Some(receipt)=transaction.query_row("SELECT object_key,etag,lower(hex(sha256)),state,generation FROM provider_object_receipts WHERE library_uuid=?1 AND binding_epoch=?2 AND generation=?3 AND object_key=?4 AND state IN ('created','already-exists-matching')",params![library_uuid,binding_epoch,transport_generation,key],|row|Ok(serde_json::json!({"objectKey":row.get::<_,String>(0)? ,"etag":row.get::<_,Option<String>>(1)? ,"sha256":row.get::<_,String>(2)? ,"state":row.get::<_,String>(3)? ,"generation":row.get::<_,i64>(4)?}))).optional()? { receipts.push(receipt); }
        }
        let plan_uuid = Uuid::now_v7().to_string();
        transaction.execute("INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,plan_uuid,created_at,updated_at) VALUES (?1,?2,?3,'stop-sync','waiting','planned',?4,?5,?6,?7,1,?8,?9,?9)",params![operation_uuid,library_uuid,root_uuid,request_hash,serde_json::to_string(request)?,i64::try_from(expected_operation_revision)?,i64::try_from(expected_binding_generation)?,plan_uuid,now])?;
        transaction.execute("INSERT INTO root_stop_plans(plan_uuid,operation_uuid,library_uuid,root_uuid,local_binding_uuid,binding_generation,root_capture_epoch,root_actor_epoch,pending_work,published_work,checkpoint_uuid,provider_id,account_uuid,remote_namespace,provider_binding_epoch,provider_transport_generation,event_frontier_json,catalog_snapshot_json,catalog_snapshot_digest,provider_receipts_json,expires_at,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",params![plan_uuid,operation_uuid,library_uuid,root_uuid,binding_uuid,generation,capture_epoch,actor_epoch,i64::try_from(work.pending_work)?,i64::try_from(work.published_work)?,checkpoint.map(|v|Uuid::from_slice(&v).map(|u|u.to_string())).transpose()?,provider_id,account_uuid,namespace,Uuid::from_slice(&binding_epoch)?.to_string(),transport_generation,serde_json::to_string(&snapshot.frontier)?,serde_json::to_string(snapshot)?,snapshot.digest,serde_json::to_string(&receipts)?,expires_at,now])?;
        transaction.commit()?;
        drop(connection);
        self.stop_sync_plan(library_uuid, root_uuid, &plan_uuid)?
            .context("stop-plan-not-found")
    }

    pub fn stop_sync_plan(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        plan_uuid: &str,
    ) -> anyhow::Result<Option<RootStopPlanRecord>> {
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row("SELECT plan_uuid,operation_uuid,root_uuid,local_binding_uuid,binding_generation,root_capture_epoch,root_actor_epoch,pending_work,published_work,checkpoint_uuid,event_frontier_json,catalog_snapshot_digest,provider_receipts_json,expires_at FROM root_stop_plans WHERE library_uuid=?1 AND root_uuid=?2 AND plan_uuid=?3",params![library_uuid,root_uuid,plan_uuid],|row|Ok(RootStopPlanRecord{plan_uuid:row.get(0)?,operation_uuid:row.get(1)?,root_uuid:row.get(2)?,local_binding_uuid:row.get(3)?,binding_generation:row.get::<_,i64>(4)?.max(0) as u64,root_capture_epoch:row.get(5)?,root_actor_epoch:row.get(6)?,pending_work:row.get::<_,i64>(7)?.max(0) as u64,published_work:row.get::<_,i64>(8)?.max(0) as u64,checkpoint_uuid:row.get(9)?,event_frontier:serde_json::from_str(&row.get::<_,String>(10)?).unwrap_or_default(),catalog_snapshot_digest:row.get(11)?,provider_receipts:serde_json::from_str(&row.get::<_,String>(12)?).unwrap_or_default(),expires_at:row.get(13)?})).optional().map_err(Into::into)
    }

    pub fn cutover_base_snapshot(
        &self,
        library_uuid: &str,
        root_uuid: &str,
    ) -> anyhow::Result<(String, crate::sync_folders::RootSnapshot, u64)> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let (snapshot_uuid,json,generation):(String,String,i64)=connection.query_row("SELECT snapshot_uuid,provider_receipts_json,binding_generation FROM root_cutover_snapshots WHERE library_uuid=?1 AND root_uuid=?2 ORDER BY created_at DESC LIMIT 1",params![library_uuid,root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).context("cutover-base-missing")?;
        let value: serde_json::Value = serde_json::from_str(&json)?;
        let snapshot = serde_json::from_value(
            value
                .get("baseSnapshot")
                .cloned()
                .context("cutover-base-corrupt")?,
        )
        .context("cutover-base-corrupt")?;
        Ok((snapshot_uuid, snapshot, generation.max(0) as u64))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_comparison_plan(
        &self,
        operation_uuid: &str,
        library_uuid: &str,
        root_uuid: &str,
        request_hash: &str,
        request: &serde_json::Value,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
        cutover_snapshot_uuid: &str,
        base: &crate::sync_folders::RootSnapshot,
        local: &crate::sync_folders::RootSnapshot,
        remote: &crate::sync_folders::RootSnapshot,
        differences: &[crate::sync_folders::SnapshotDifference],
        account_generation: u64,
        provider_generation: u64,
    ) -> anyhow::Result<RootComparisonPlanRecord> {
        let now = unix_timestamp_millis();
        let expires_at = now + crate::sync_folders::PLAN_TTL_MILLIS;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some(plan_uuid) = transaction
            .query_row(
                "SELECT plan_uuid FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
        {
            transaction.commit()?;
            drop(connection);
            return self
                .root_comparison_plan(library_uuid, root_uuid, &plan_uuid)?
                .context("comparison-plan-not-found");
        }
        let binding:(String,i64,i64)=transaction.query_row("SELECT state,binding_generation,operation_revision FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",params![library_uuid,root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).context("local-root-binding-not-found")?;
        if binding.0 != "local-only-fork" {
            bail!("comparison-required");
        }
        if binding.1 != i64::try_from(expected_binding_generation)? {
            bail!("stale-binding-generation");
        }
        if binding.2 != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        let request_json = serde_json::to_string(request)?;
        let plan_uuid = Uuid::now_v7().to_string();
        transaction.execute("INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,plan_uuid,created_at,updated_at) VALUES (?1,?2,?3,'re-enable','waiting','comparison-ready',?4,?5,?6,?7,1,?8,?9,?9)",params![operation_uuid,library_uuid,root_uuid,request_hash,request_json,i64::try_from(expected_operation_revision)?,i64::try_from(expected_binding_generation)?,plan_uuid,now])?;
        let conflict_count = differences
            .iter()
            .filter(|item| item.classification == "conflict")
            .count();
        transaction.execute("INSERT INTO root_comparison_plans(plan_uuid,operation_uuid,cutover_snapshot_uuid,library_uuid,root_uuid,base_digest,local_digest,remote_digest,base_frontier_json,local_frontier_json,remote_frontier_json,root_revision,account_generation,provider_generation,item_count,conflict_count,status,expires_at,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,'ready',?17,?18,?18)",params![plan_uuid,operation_uuid,cutover_snapshot_uuid,library_uuid,root_uuid,base.digest,local.digest,remote.digest,serde_json::to_string(&base.frontier)?,serde_json::to_string(&local.frontier)?,serde_json::to_string(&remote.frontier)?,i64::try_from(expected_operation_revision)?,i64::try_from(account_generation)?,i64::try_from(provider_generation)?,i64::try_from(differences.len())?,i64::try_from(conflict_count)?,expires_at,now])?;
        transaction.execute(
            "INSERT INTO root_comparison_snapshots(plan_uuid,base_snapshot_json,local_snapshot_json,remote_snapshot_json) VALUES (?1,?2,?3,?4)",
            params![plan_uuid,serde_json::to_string(base)?,serde_json::to_string(local)?,serde_json::to_string(remote)?],
        )?;
        for (ordinal, item) in differences.iter().enumerate() {
            transaction.execute("INSERT INTO root_comparison_items(plan_uuid,item_ordinal,entity_uuid,field_name,classification,base_digest,local_digest,remote_digest,suggested_action,conflict_uuid,redacted_detail_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![plan_uuid,i64::try_from(ordinal)?,item.entity_uuid,item.field_name,item.classification,item.base_digest,item.local_digest,item.remote_digest,item.suggested_action,item.conflict_uuid,serde_json::to_string(&serde_json::json!({"entityUuid":item.entity_uuid,"fieldName":item.field_name}))?])?;
        }
        transaction.commit()?;
        drop(connection);
        self.root_comparison_plan(library_uuid, root_uuid, &plan_uuid)?
            .context("comparison-plan-not-found")
    }

    pub fn root_comparison_plan(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        plan_uuid: &str,
    ) -> anyhow::Result<Option<RootComparisonPlanRecord>> {
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row("SELECT plan_uuid,operation_uuid,root_uuid,base_digest,local_digest,remote_digest,base_frontier_json,local_frontier_json,remote_frontier_json,root_revision,account_generation,provider_generation,status,item_count,conflict_count,chosen_mode,expires_at FROM root_comparison_plans WHERE library_uuid=?1 AND root_uuid=?2 AND plan_uuid=?3",params![library_uuid,root_uuid,plan_uuid],|row|Ok(RootComparisonPlanRecord{plan_uuid:row.get(0)?,operation_uuid:row.get(1)?,root_uuid:row.get(2)?,base_digest:row.get(3)?,local_digest:row.get(4)?,remote_digest:row.get(5)?,base_frontier:serde_json::from_str(&row.get::<_,String>(6)?).unwrap_or(serde_json::json!({})),local_frontier:serde_json::from_str(&row.get::<_,String>(7)?).unwrap_or(serde_json::json!({})),remote_frontier:serde_json::from_str(&row.get::<_,String>(8)?).unwrap_or(serde_json::json!({})),root_revision:row.get::<_,i64>(9)?.max(0) as u64,account_generation:row.get::<_,i64>(10)?.max(0) as u64,provider_generation:row.get::<_,i64>(11)?.max(0) as u64,status:row.get(12)?,item_count:row.get::<_,i64>(13)?.max(0) as u64,conflict_count:row.get::<_,i64>(14)?.max(0) as u64,chosen_mode:row.get(15)?,expires_at:row.get(16)?})).optional().map_err(Into::into)
    }

    pub fn root_comparison_items(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        plan_uuid: &str,
        after: u64,
        limit: u64,
    ) -> anyhow::Result<Vec<RootComparisonItemRecord>> {
        if self
            .root_comparison_plan(library_uuid, root_uuid, plan_uuid)?
            .is_none()
        {
            bail!("comparison-plan-not-found");
        }
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement=connection.prepare("SELECT item.item_ordinal,item.entity_uuid,item.field_name,item.classification,item.suggested_action,resolution.resolution,item.conflict_uuid,item.redacted_detail_json FROM root_comparison_items item LEFT JOIN root_comparison_resolutions resolution ON resolution.plan_uuid=item.plan_uuid AND resolution.item_ordinal=item.item_ordinal WHERE item.plan_uuid=?1 AND item.item_ordinal>=?2 ORDER BY item.item_ordinal LIMIT ?3")?;
        let rows = statement.query_map(
            params![
                plan_uuid,
                i64::try_from(after)?,
                i64::try_from(limit.min(500))?
            ],
            |row| {
                Ok(RootComparisonItemRecord {
                    item_ordinal: row.get::<_, i64>(0)?.max(0) as u64,
                    entity_uuid: row.get(1)?,
                    field_name: row.get(2)?,
                    classification: row.get(3)?,
                    suggested_action: row.get(4)?,
                    resolution: row.get(5)?,
                    conflict_uuid: row.get(6)?,
                    redacted_detail: row
                        .get::<_, Option<String>>(7)?
                        .and_then(|v| serde_json::from_str(&v).ok()),
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn resolve_comparison_item(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        plan_uuid: &str,
        item_ordinal: u64,
        operation_uuid: &str,
        resolution: &str,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
    ) -> anyhow::Result<RootComparisonItemRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let facts:(i64,i64,String,i64,String)=transaction.query_row("SELECT binding.operation_revision,binding.binding_generation,binding.state,plan.expires_at,plan.operation_uuid FROM root_comparison_plans plan JOIN local_root_bindings binding ON binding.library_uuid=plan.library_uuid AND binding.root_uuid=plan.root_uuid WHERE plan.library_uuid=?1 AND plan.root_uuid=?2 AND plan.plan_uuid=?3 AND plan.status='ready'",params![library_uuid,root_uuid,plan_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).context("comparison-plan-not-found")?;
        if facts.0 != i64::try_from(expected_operation_revision)?
            || facts.1 != i64::try_from(expected_binding_generation)?
            || facts.2 != "local-only-fork"
            || facts.3 < now
            || facts.4 != operation_uuid
        {
            bail!("comparison-plan-stale");
        }
        let classification:String=transaction.query_row("SELECT classification FROM root_comparison_items WHERE plan_uuid=?1 AND item_ordinal=?2",params![plan_uuid,i64::try_from(item_ordinal)?],|row|row.get(0)).context("comparison-item-not-found")?;
        if classification != "conflict" {
            bail!("comparison-resolution-not-required");
        }
        transaction.execute("INSERT INTO root_comparison_resolutions(plan_uuid,item_ordinal,resolution,resolved_at) VALUES (?1,?2,?3,?4) ON CONFLICT(plan_uuid,item_ordinal) DO UPDATE SET resolution=excluded.resolution,resolved_at=excluded.resolved_at",params![plan_uuid,i64::try_from(item_ordinal)?,resolution,now])?;
        transaction.commit()?;
        drop(connection);
        self.root_comparison_items(library_uuid, root_uuid, plan_uuid, item_ordinal, 1)?
            .into_iter()
            .next()
            .context("comparison-item-not-found")
    }

    pub fn sync_transport_generations(&self, library_uuid: &str) -> anyhow::Result<(u64, u64)> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let (account,provider):(i64,i64)=connection.query_row("SELECT account.generation,binding.transport_generation FROM library_sync_bindings binding JOIN plugin_accounts account ON account.account_uuid=binding.account_uuid WHERE binding.library_uuid=?1",[library_uuid],|row|Ok((row.get(0)?,row.get(1)?))).optional()?.unwrap_or((0,0));
        Ok((account.max(0) as u64, provider.max(0) as u64))
    }

    pub fn set_root_binding_state(
        &self,
        operation_uuid: &str,
        library_uuid: &str,
        root_uuid: &str,
        command: &str,
        request_hash: &str,
        request: &serde_json::Value,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
    ) -> anyhow::Result<RootOperationRecord> {
        let (kind, next_state) = match command {
            "pause" => ("select", "paused"),
            "resume" => ("select", "active"),
            _ => bail!("invalid-root-command"),
        };
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some(existing)=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row).optional()? { transaction.commit()?; return Ok(existing); }
        let binding:(String,i64,i64)=transaction.query_row("SELECT state,binding_generation,operation_revision FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",params![library_uuid,root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).context("local-root-binding-not-found")?;
        if binding.1 != i64::try_from(expected_binding_generation)? {
            bail!("stale-binding-generation");
        }
        if binding.2 != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        if (command == "pause" && binding.0 != "active")
            || (command == "resume" && binding.0 != "paused")
        {
            bail!("invalid-root-state");
        }
        transaction.execute("INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,created_at,updated_at) VALUES (?1,?2,?3,?4,'completed','completed',?5,?6,?7,?8,1,?9,?9)",params![operation_uuid,library_uuid,root_uuid,kind,request_hash,serde_json::to_string(request)?,i64::try_from(expected_operation_revision)?,i64::try_from(expected_binding_generation)?,now])?;
        transaction.execute("UPDATE local_root_bindings SET state=?1,operation_revision=operation_revision+1,updated_at=?2 WHERE library_uuid=?3 AND root_uuid=?4",params![next_state,now,library_uuid,root_uuid])?;
        let result=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row)?;
        transaction.commit()?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_reenable(
        &self,
        operation_uuid: &str,
        library_uuid: &str,
        root_uuid: &str,
        plan_uuid: &str,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
        local_digest: &str,
        remote_digest: &str,
        account_generation: u64,
        provider_generation: u64,
        mode: &str,
    ) -> anyhow::Result<(RootOperationRecord, LocalRootBindingRecord)> {
        if mode != "merge-local-fork" {
            bail!("invalid-comparison-mode");
        }
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let operation=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row).context("root-operation-not-found")?;
        if matches!(operation.state.as_str(), "running" | "completed") {
            let operation_plan: Option<String> = transaction.query_row(
                "SELECT plan_uuid FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| row.get(0),
            )?;
            if operation.library_uuid != library_uuid
                || operation.root_uuid != root_uuid
                || operation.kind != "re-enable"
                || operation_plan.as_deref() != Some(plan_uuid)
            {
                bail!("idempotency-key-reused");
            }
            transaction.commit()?;
            drop(connection);
            return Ok((
                operation,
                self.local_root_binding(library_uuid, root_uuid)?
                    .context("local-root-binding-not-found")?,
            ));
        }
        let binding:(String,i64,i64)=transaction.query_row("SELECT state,binding_generation,operation_revision FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",params![library_uuid,root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).context("local-root-binding-not-found")?;
        let plan:(String,String,String,i64,i64,i64,i64,String)=transaction.query_row("SELECT local_digest,remote_digest,status,root_revision,account_generation,provider_generation,expires_at,operation_uuid FROM root_comparison_plans WHERE library_uuid=?1 AND root_uuid=?2 AND plan_uuid=?3",params![library_uuid,root_uuid,plan_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).context("comparison-plan-not-found")?;
        let unresolved:i64=transaction.query_row("SELECT COUNT(*) FROM root_comparison_items item WHERE item.plan_uuid=?1 AND item.classification='conflict' AND NOT EXISTS(SELECT 1 FROM root_comparison_resolutions resolution WHERE resolution.plan_uuid=item.plan_uuid AND resolution.item_ordinal=item.item_ordinal)",[plan_uuid],|row|row.get(0))?;
        if binding.0 != "local-only-fork"
            || binding.1 != i64::try_from(expected_binding_generation)?
            || binding.2 != i64::try_from(expected_operation_revision)?
            || plan.0 != local_digest
            || plan.1 != remote_digest
            || plan.2 != "ready"
            || plan.3 != i64::try_from(expected_operation_revision)?
            || plan.4 != i64::try_from(account_generation)?
            || plan.5 != i64::try_from(provider_generation)?
            || plan.6 < now
            || plan.7 != operation_uuid
            || unresolved != 0
        {
            bail!("comparison-plan-stale");
        }
        let new_binding_uuid = Uuid::now_v7().to_string();
        let root_capture_epoch = Uuid::now_v7().to_string();
        let root_actor_epoch = Uuid::now_v7().to_string();
        let snapshots:(String,String)=transaction.query_row(
            "SELECT local_snapshot_json,remote_snapshot_json FROM root_comparison_snapshots WHERE plan_uuid=?1",
            [plan_uuid],
            |row| Ok((row.get(0)?,row.get(1)?)),
        ).context("comparison-plan-snapshots-missing")?;
        let local_snapshot: crate::sync_folders::RootSnapshot = serde_json::from_str(&snapshots.0)?;
        let remote_snapshot: crate::sync_folders::RootSnapshot =
            serde_json::from_str(&snapshots.1)?;
        transaction.execute("UPDATE local_root_bindings SET local_binding_uuid=?1,binding_generation=binding_generation+1,root_capture_epoch=?2,root_actor_epoch=?3,state='reconcile-required',operation_revision=operation_revision+1,last_error_code=NULL,last_error_detail=NULL,updated_at=?4 WHERE library_uuid=?5 AND root_uuid=?6",params![new_binding_uuid,root_capture_epoch,root_actor_epoch,now,library_uuid,root_uuid])?;
        let mut statement=transaction.prepare("SELECT item.item_ordinal,item.entity_uuid,item.field_name,item.classification,COALESCE(resolution.resolution,item.suggested_action) FROM root_comparison_items item LEFT JOIN root_comparison_resolutions resolution ON resolution.plan_uuid=item.plan_uuid AND resolution.item_ordinal=item.item_ordinal WHERE item.plan_uuid=?1 AND item.classification<>'unchanged' ORDER BY item.item_ordinal")?;
        let items = statement
            .query_map([plan_uuid], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for (ordinal, entity, field, classification, resolution) in items {
            let resolution = resolution
                .or_else(|| match classification.as_str() {
                    "local-only" | "deleted-local" | "same-change" => Some("keep-local".to_owned()),
                    "remote-only" | "deleted-remote" => Some("keep-remote".to_owned()),
                    _ => None,
                })
                .context("comparison-resolution-required")?;
            let local_value = local_snapshot
                .entities
                .get(&entity)
                .and_then(|fields| fields.get(&field))
                .cloned();
            let remote_value = remote_snapshot
                .entities
                .get(&entity)
                .and_then(|fields| fields.get(&field))
                .cloned();
            let selected_value = match resolution.as_str() {
                "keep-local" => local_value.clone(),
                "keep-remote" => remote_value.clone(),
                "keep-both" => None,
                _ => bail!("invalid-comparison-resolution"),
            };
            let selected_entity = match resolution.as_str() {
                "keep-local" => local_snapshot.entities.get(&entity),
                "keep-remote" => remote_snapshot.entities.get(&entity),
                "keep-both" => {
                    let local = local_snapshot.entities.get(&entity);
                    let remote = remote_snapshot.entities.get(&entity);
                    transaction.execute("INSERT INTO root_reconciliation_work(plan_uuid,item_ordinal,entity_uuid,field_name,resolution,selected_value_json,selected_entity_json,apply_order,state,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9)",params![plan_uuid,ordinal,entity,field,resolution,serde_json::to_string(&serde_json::json!({"local":local_value,"remote":remote_value}))?,serde_json::to_string(&serde_json::json!({"local":local,"remote":remote}))?,i64::try_from(reconciliation_apply_order(&local_snapshot,&entity,&field,ordinal.max(0) as u64))?,now])?;
                    continue;
                }
                _ => None,
            }
            .map(serde_json::to_string)
            .transpose()?;
            let selected_snapshot = if resolution == "keep-remote" {
                &remote_snapshot
            } else {
                &local_snapshot
            };
            let apply_order = reconciliation_apply_order(
                selected_snapshot,
                &entity,
                &field,
                ordinal.max(0) as u64,
            );
            transaction.execute("INSERT INTO root_reconciliation_work(plan_uuid,item_ordinal,entity_uuid,field_name,resolution,selected_value_json,selected_entity_json,apply_order,state,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9)",params![plan_uuid,ordinal,entity,field,resolution,selected_value,selected_entity,i64::try_from(apply_order)?,now])?;
            let _ = classification;
        }
        transaction.execute("UPDATE root_comparison_plans SET chosen_mode=?1,status='committed',updated_at=?2 WHERE plan_uuid=?3",params![mode,now,plan_uuid])?;
        transaction.execute("UPDATE root_operations SET state='running',phase='reconciling',operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2",params![now,operation_uuid])?;
        transaction.execute("UPDATE remote_sync_roots SET operation_revision=operation_revision+1,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3",params![now,library_uuid,root_uuid])?;
        let completed=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row)?;
        transaction.commit()?;
        drop(connection);
        Ok((
            completed,
            self.local_root_binding(library_uuid, root_uuid)?
                .context("local-root-binding-not-found")?,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_publish_as_new(
        &self,
        operation_uuid: &str,
        library_uuid: &str,
        source_root_uuid: &str,
        plan_uuid: &str,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
        local_digest: &str,
        remote_digest: &str,
        account_generation: u64,
        provider_generation: u64,
    ) -> anyhow::Result<(RootOperationRecord, RootPublishAsNewRecord)> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some(existing) = transaction
            .query_row(
                "SELECT operation_uuid,enrollment_operation_uuid,library_uuid,source_root_uuid,new_root_uuid,new_root_folder_uuid,new_local_binding_uuid,root_capture_epoch,root_actor_epoch FROM root_publish_as_new WHERE operation_uuid=?1",
                [operation_uuid],
                root_publish_as_new_record_from_row,
            )
            .optional()?
        {
            if existing.library_uuid != library_uuid || existing.source_root_uuid != source_root_uuid {
                bail!("idempotency-key-reused");
            }
            let operation = transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                root_operation_record_from_row,
            )?;
            transaction.commit()?;
            return Ok((operation, existing));
        }
        let operation=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row).context("root-operation-not-found")?;
        let binding:LocalRootBindingRecord=transaction.query_row(
            "SELECT library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,binding_generation,root_capture_epoch,root_actor_epoch,desired_materialized_name,desired_materialization_fingerprint,applied_materialized_name,applied_materialization_fingerprint,operation_revision,last_error_code FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
            params![library_uuid,source_root_uuid],
            local_root_binding_record_from_row,
        ).context("local-root-binding-not-found")?;
        let descriptor:RemoteSyncRootRecord=transaction.query_row(
            "SELECT remote.library_uuid,(SELECT root_uuid FROM libraries WHERE library_uuid=remote.library_uuid),remote.root_uuid,remote.root_folder_uuid,remote.requested_name,remote.materialized_name,remote.materialization_fingerprint,remote.logical_revision,remote.lifecycle,remote.inventory_state,remote.logical_bytes,remote.checkpoint_uuid,remote.inventory_revision,remote.operation_revision FROM remote_sync_roots remote WHERE remote.library_uuid=?1 AND remote.root_uuid=?2",
            params![library_uuid,source_root_uuid],
            remote_sync_root_record_from_row,
        ).context("root-not-found")?;
        let plan:(String,String,String,i64,i64,i64,i64,String)=transaction.query_row("SELECT local_digest,remote_digest,status,root_revision,account_generation,provider_generation,expires_at,operation_uuid FROM root_comparison_plans WHERE library_uuid=?1 AND root_uuid=?2 AND plan_uuid=?3",params![library_uuid,source_root_uuid,plan_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).context("comparison-plan-not-found")?;
        if operation.library_uuid != library_uuid
            || operation.root_uuid != source_root_uuid
            || operation.kind != "re-enable"
            || operation.state != "waiting"
            || binding.state != "local-only-fork"
            || binding.binding_generation != expected_binding_generation
            || binding.operation_revision != expected_operation_revision
            || plan.0 != local_digest
            || plan.1 != remote_digest
            || plan.2 != "ready"
            || plan.3 != i64::try_from(expected_operation_revision)?
            || plan.4 != i64::try_from(account_generation)?
            || plan.5 != i64::try_from(provider_generation)?
            || plan.6 < now
            || plan.7 != operation_uuid
        {
            bail!("comparison-plan-stale");
        }
        let new_root_uuid = Uuid::now_v7().to_string();
        let new_root_folder_uuid = Uuid::now_v7().to_string();
        let new_local_binding_uuid = Uuid::now_v7().to_string();
        let root_capture_epoch = Uuid::now_v7().to_string();
        let root_actor_epoch = Uuid::now_v7().to_string();
        let enrollment_operation_uuid = Uuid::now_v7().to_string();
        let occupied = {
            let mut statement=transaction.prepare("SELECT materialized_name FROM remote_sync_roots WHERE library_uuid=?1 AND lifecycle='active'")?;
            let names = statement
                .query_map([library_uuid], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            names
                .into_iter()
                .map(|name| crate::sync::reducer::materialized_name_compare_key(&name))
                .collect::<std::collections::BTreeSet<_>>()
        };
        let materialized_name = crate::sync_folders::default_materialized_name(
            &descriptor.requested_name,
            Uuid::parse_str(&new_root_uuid)?,
            &occupied,
        );
        let fingerprint = crate::sync_folders::materialization_fingerprint(&materialized_name);
        transaction.execute("INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,logical_revision,lifecycle,inventory_state,logical_bytes,checkpoint_uuid,inventory_revision,operation_revision,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,0,'active','unselected',?8,NULL,1,1,?9)",params![library_uuid,new_root_uuid,new_root_folder_uuid,descriptor.requested_name,materialized_name,materialized_name.to_lowercase(),fingerprint,descriptor.logical_bytes.map(i64::try_from).transpose()?,now])?;
        transaction.execute("DELETE FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2 AND state='local-only-fork' AND binding_generation=?3",params![library_uuid,source_root_uuid,i64::try_from(expected_binding_generation)?])?;
        transaction.execute("INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,marker_identity,filesystem_identity,state,binding_generation,root_capture_epoch,root_actor_epoch,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,applied_materialized_name,applied_materialized_name_key,applied_materialization_fingerprint,operation_revision,verification_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,NULL,'materializing',1,?8,?9,?10,?11,?12,?10,?11,?12,1,?13,?14,?14)",params![library_uuid,new_root_uuid,new_local_binding_uuid,binding.local_path,binding.canonical_path_key,binding.placement_parent,binding.placement_parent_key,root_capture_epoch,root_actor_epoch,materialized_name,materialized_name.to_lowercase(),fingerprint,serde_json::to_string(&serde_json::json!({"sourceRootUuid":source_root_uuid,"comparisonPlanUuid":plan_uuid}))?,now])?;
        let enrollment_request = serde_json::json!({"source":"publish-as-new","sourceOperationUuid":operation_uuid,"comparisonPlanUuid":plan_uuid,"localPath":binding.local_path});
        let enrollment_hash = crate::sync_folders::request_hash(&enrollment_request)?;
        transaction.execute("INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,plan_uuid,created_at,updated_at) VALUES (?1,?2,?3,'enroll','running','capturing-seed',?4,?5,0,NULL,1,?6,?7,?7)",params![enrollment_operation_uuid,library_uuid,new_root_uuid,enrollment_hash,serde_json::to_string(&enrollment_request)?,plan_uuid,now])?;
        transaction.execute("INSERT INTO root_publish_as_new(operation_uuid,enrollment_operation_uuid,library_uuid,source_root_uuid,new_root_uuid,new_root_folder_uuid,new_local_binding_uuid,root_capture_epoch,root_actor_epoch,state,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'catalog-pending',?10,?10)",params![operation_uuid,enrollment_operation_uuid,library_uuid,source_root_uuid,new_root_uuid,new_root_folder_uuid,new_local_binding_uuid,root_capture_epoch,root_actor_epoch,now])?;
        transaction.execute("UPDATE root_comparison_plans SET chosen_mode='publish-as-new',status='committed',updated_at=?1 WHERE plan_uuid=?2",params![now,plan_uuid])?;
        transaction.execute("UPDATE root_operations SET state='running',phase='materializing',cutover_committed=1,operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2",params![now,operation_uuid])?;
        transaction.execute("UPDATE remote_sync_roots SET operation_revision=operation_revision+1,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3",params![now,library_uuid,source_root_uuid])?;
        let result=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row)?;
        let publish = RootPublishAsNewRecord {
            operation_uuid: operation_uuid.to_owned(),
            enrollment_operation_uuid,
            library_uuid: library_uuid.to_owned(),
            source_root_uuid: source_root_uuid.to_owned(),
            new_root_uuid,
            new_root_folder_uuid,
            new_local_binding_uuid,
            root_capture_epoch,
            root_actor_epoch,
        };
        transaction.commit()?;
        Ok((result, publish))
    }

    pub fn pending_publish_as_new(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Vec<RootPublishAsNewRecord>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement=connection.prepare("SELECT operation_uuid,enrollment_operation_uuid,library_uuid,source_root_uuid,new_root_uuid,new_root_folder_uuid,new_local_binding_uuid,root_capture_epoch,root_actor_epoch FROM root_publish_as_new WHERE library_uuid=?1 AND state='catalog-pending' ORDER BY created_at,operation_uuid")?;
        let rows = statement.query_map([library_uuid], root_publish_as_new_record_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn publish_as_new(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<Option<RootPublishAsNewRecord>> {
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row("SELECT operation_uuid,enrollment_operation_uuid,library_uuid,source_root_uuid,new_root_uuid,new_root_folder_uuid,new_local_binding_uuid,root_capture_epoch,root_actor_epoch FROM root_publish_as_new WHERE operation_uuid=?1",[operation_uuid],root_publish_as_new_record_from_row).optional().map_err(Into::into)
    }

    pub fn complete_publish_as_new(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let publish:RootPublishAsNewRecord=transaction.query_row("SELECT operation_uuid,enrollment_operation_uuid,library_uuid,source_root_uuid,new_root_uuid,new_root_folder_uuid,new_local_binding_uuid,root_capture_epoch,root_actor_epoch FROM root_publish_as_new WHERE operation_uuid=?1",[operation_uuid],root_publish_as_new_record_from_row).context("publish-as-new-not-found")?;
        transaction.execute("UPDATE local_root_bindings SET state='active',operation_revision=operation_revision+1,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3 AND local_binding_uuid=?4 AND state IN ('materializing','active')",params![now,publish.library_uuid,publish.new_root_uuid,publish.new_local_binding_uuid])?;
        transaction.execute("UPDATE root_operations SET state='completed',phase='completed',operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2 AND state<>'completed'",params![now,publish.enrollment_operation_uuid])?;
        transaction.execute("UPDATE root_operations SET state='completed',phase='completed',result_json=?1,operation_revision=operation_revision+1,updated_at=?2 WHERE operation_uuid=?3 AND state<>'completed'",params![serde_json::to_string(&serde_json::json!({"newRootUuid":publish.new_root_uuid,"newRootFolderUuid":publish.new_root_folder_uuid,"enrollmentOperationUuid":publish.enrollment_operation_uuid}))?,now,operation_uuid])?;
        transaction.execute(
            "UPDATE root_publish_as_new SET state='complete',updated_at=?1 WHERE operation_uuid=?2",
            params![now, operation_uuid],
        )?;
        let result=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn pending_root_reconciliation_work(
        &self,
        library_uuid: &str,
        root_uuid: &str,
    ) -> anyhow::Result<Vec<RootReconciliationWorkRecord>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement=connection.prepare("SELECT work.plan_uuid,work.item_ordinal,work.entity_uuid,work.field_name,work.resolution,work.selected_value_json,work.selected_entity_json,binding.local_binding_uuid,binding.binding_generation FROM root_reconciliation_work work JOIN root_comparison_plans plan ON plan.plan_uuid=work.plan_uuid JOIN local_root_bindings binding ON binding.library_uuid=plan.library_uuid AND binding.root_uuid=plan.root_uuid WHERE plan.library_uuid=?1 AND plan.root_uuid=?2 AND work.state IN ('pending','applying','failed') AND binding.state='reconcile-required' ORDER BY work.apply_order,work.item_ordinal")?;
        let rows = statement.query_map(params![library_uuid, root_uuid], |row| {
            Ok(RootReconciliationWorkRecord {
                plan_uuid: row.get(0)?,
                item_ordinal: row.get::<_, i64>(1)?.max(0) as u64,
                entity_uuid: row.get(2)?,
                field_name: row.get(3)?,
                resolution: row.get(4)?,
                selected_value: row.get(5)?,
                selected_entity: row.get(6)?,
                local_binding_uuid: row.get(7)?,
                binding_generation: row.get::<_, i64>(8)?.max(0) as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn complete_root_reconciliation_item(
        &self,
        plan_uuid: &str,
        item_ordinal: u64,
    ) -> anyhow::Result<()> {
        let changed=self.connection.lock().unwrap_or_else(|p|p.into_inner()).execute("UPDATE root_reconciliation_work SET state='complete',last_error_code=NULL,updated_at=?1 WHERE plan_uuid=?2 AND item_ordinal=?3 AND state IN ('pending','applying','failed','complete')",params![unix_timestamp_millis(),plan_uuid,i64::try_from(item_ordinal)?])?;
        if changed != 1 {
            bail!("reconciliation-work-not-found");
        }
        Ok(())
    }

    pub fn fail_root_reconciliation_item(
        &self,
        plan_uuid: &str,
        item_ordinal: u64,
        error_code: &str,
    ) -> anyhow::Result<()> {
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).execute("UPDATE root_reconciliation_work SET state='failed',attempt_count=attempt_count+1,last_error_code=?1,updated_at=?2 WHERE plan_uuid=?3 AND item_ordinal=?4",params![error_code,unix_timestamp_millis(),plan_uuid,i64::try_from(item_ordinal)?])?;
        Ok(())
    }

    pub fn complete_root_reconciliation(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        plan_uuid: &str,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let pending:i64=transaction.query_row("SELECT COUNT(*) FROM root_reconciliation_work WHERE plan_uuid=?1 AND state<>'complete'",[plan_uuid],|row|row.get(0))?;
        if pending != 0 {
            bail!("reconciliation-incomplete");
        }
        let operation_uuid:String=transaction.query_row("SELECT operation_uuid FROM root_comparison_plans WHERE plan_uuid=?1 AND library_uuid=?2 AND root_uuid=?3",params![plan_uuid,library_uuid,root_uuid],|row|row.get(0))?;
        transaction.execute("UPDATE local_root_bindings SET state='active',operation_revision=operation_revision+1,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3 AND state='reconcile-required'",params![now,library_uuid,root_uuid])?;
        transaction.execute("UPDATE root_operations SET state='completed',phase='completed',operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2 AND state='running' AND phase='reconciling'",params![now,operation_uuid])?;
        let result=transaction.query_row("SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",[operation_uuid],root_operation_record_from_row)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn root_operation_result(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let json: Option<String> = connection
            .query_row(
                "SELECT result_json FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        json.map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    pub fn store_root_path_preflight(
        &self,
        token: &str,
        library_uuid: &str,
        root_uuid: Option<&str>,
        mode: &str,
        request_hash: &str,
        request: &serde_json::Value,
        result: &serde_json::Value,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        let request_json = serde_json::to_string(request)?;
        let result_json = serde_json::to_string(result)?;
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = connection.query_row(
            "SELECT library_uuid,root_uuid,mode,request_hash,request_json FROM root_path_preflights WHERE preflight_token=?1",
            [token],
            |row| Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?)),
        ).optional()? {
            if existing != (library_uuid.to_owned(),root_uuid.map(str::to_owned),mode.to_owned(),request_hash.to_owned(),request_json) {
                bail!("idempotency-key-reused");
            }
            return Ok(());
        }
        connection.execute(
            "INSERT INTO root_path_preflights(preflight_token,library_uuid,root_uuid,mode,request_hash,request_json,result_json,expires_at,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![token,library_uuid,root_uuid,mode,request_hash,request_json,result_json,expires_at,unix_timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn root_path_preflight(
        &self,
        token: &str,
        library_uuid: &str,
    ) -> anyhow::Result<(
        Option<String>,
        String,
        String,
        serde_json::Value,
        serde_json::Value,
        i64,
        Option<i64>,
    )> {
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row(
            "SELECT root_uuid,mode,request_hash,request_json,result_json,expires_at,consumed_at FROM root_path_preflights WHERE preflight_token=?1 AND library_uuid=?2",
            params![token,library_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,serde_json::from_str(&row.get::<_,String>(3)?).unwrap_or(serde_json::Value::Null),serde_json::from_str(&row.get::<_,String>(4)?).unwrap_or(serde_json::Value::Null),row.get(5)?,row.get(6)?)),
        ).context("preflight-not-found")
    }

    pub fn consume_root_path_preflight(
        &self,
        token: &str,
        library_uuid: &str,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let changed=self.connection.lock().unwrap_or_else(|p|p.into_inner()).execute("UPDATE root_path_preflights SET consumed_at=COALESCE(consumed_at,?1) WHERE preflight_token=?2 AND library_uuid=?3 AND expires_at>=?1",params![now,token,library_uuid])?;
        if changed != 1 {
            bail!("stale-plan");
        }
        Ok(())
    }

    pub fn refresh_started_root_path_preflight(
        &self,
        token: &str,
        library_uuid: &str,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        let changed = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .execute(
                "UPDATE root_path_preflights SET expires_at=MAX(expires_at,?1) WHERE preflight_token=?2 AND library_uuid=?3 AND EXISTS(SELECT 1 FROM root_operations WHERE operation_uuid=?2 AND library_uuid=?3 AND kind='enroll' AND state IN ('pending','running','waiting'))",
                params![expires_at,token,library_uuid],
            )?;
        if changed != 1 {
            bail!("stale-plan");
        }
        Ok(())
    }

    pub fn commit_local_root_binding(
        &self,
        operation_uuid: &str,
        plan_uuid: &str,
        binding: &NewLocalRootBinding,
    ) -> anyhow::Result<LocalRootBindingRecord> {
        if binding.applied_materialized_name.is_some()
            != binding.applied_materialization_fingerprint.is_some()
        {
            bail!("invalid-root-materialization");
        }
        let generation = i64::try_from(binding.binding_generation)?;
        let revision = i64::try_from(binding.operation_revision)?;
        let now = unix_timestamp_millis();
        let verification_json = serde_json::to_string(&binding.verification_json)?;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let operation: (String,String,String,String,String) = transaction.query_row(
            "SELECT library_uuid,root_uuid,kind,state,COALESCE(plan_uuid,'') FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).context("root-operation-not-found")?;
        if operation.0 != binding.library_uuid
            || operation.1 != binding.root_uuid
            || operation.4 != plan_uuid
        {
            bail!("stale-plan");
        }
        let enrollment = operation.2 == "enroll";
        if enrollment && binding.applied_materialized_name.is_none() {
            bail!("invalid-root-materialization");
        }
        if operation.3 == "completed" && !enrollment {
            transaction.commit()?;
            drop(connection);
            return self
                .local_root_binding(&binding.library_uuid, &binding.root_uuid)?
                .context("local-root-binding-not-found");
        }
        if operation.3 != "waiting" && operation.3 != "completed" {
            bail!("operation-in-progress");
        }
        let previous: Option<(String,i64,String,String,String)> = transaction.query_row(
            "SELECT state,binding_generation,local_binding_uuid,local_path,canonical_path_key FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
            params![binding.library_uuid,binding.root_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).optional()?;
        let sealed_lineage: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM root_cutover_snapshots WHERE library_uuid=?1 AND root_uuid=?2)",
            params![binding.library_uuid,binding.root_uuid],
            |row| row.get(0),
        )?;
        if previous
            .as_ref()
            .is_some_and(|(state, _, _, _, _)| state == "local-only-fork")
            || sealed_lineage
        {
            bail!("comparison-required");
        }
        if let Some((_, previous_generation, previous_binding_uuid, previous_path, previous_key)) =
            previous
        {
            let replaying_enrollment = enrollment
                && generation == previous_generation
                && binding.local_binding_uuid == previous_binding_uuid
                && binding.local_path == previous_path
                && binding.canonical_path_key == previous_key;
            if generation < previous_generation
                || (generation == previous_generation && !replaying_enrollment)
            {
                bail!("stale-binding-generation");
            }
            transaction.execute(
                "UPDATE local_root_bindings SET local_binding_uuid=?1,local_path=?2,canonical_path_key=?3,placement_parent=?4,placement_parent_key=?5,marker_identity=?6,filesystem_identity=?7,state=?8,binding_generation=?9,root_capture_epoch=?10,root_actor_epoch=?11,desired_materialized_name=?12,desired_materialized_name_key=?13,desired_materialization_fingerprint=?14,applied_materialized_name=COALESCE(?15,applied_materialized_name),applied_materialized_name_key=CASE WHEN ?15 IS NULL THEN applied_materialized_name_key ELSE lower(?15) END,applied_materialization_fingerprint=COALESCE(?16,applied_materialization_fingerprint),operation_revision=?17,verification_json=?18,last_error_code=NULL,last_error_detail=NULL,updated_at=?19 WHERE library_uuid=?20 AND root_uuid=?21",
                params![binding.local_binding_uuid,binding.local_path,binding.canonical_path_key,binding.placement_parent,binding.placement_parent_key,binding.marker_identity,binding.filesystem_identity,binding.state,generation,binding.root_capture_epoch,binding.root_actor_epoch,binding.desired_materialized_name,binding.desired_materialized_name.to_lowercase(),binding.desired_materialization_fingerprint,binding.applied_materialized_name,binding.applied_materialization_fingerprint,revision,verification_json,now,binding.library_uuid,binding.root_uuid],
            )?;
        } else {
            transaction.execute(
                "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,marker_identity,filesystem_identity,state,binding_generation,root_capture_epoch,root_actor_epoch,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,applied_materialized_name,applied_materialized_name_key,applied_materialization_fingerprint,operation_revision,verification_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,CASE WHEN ?17 IS NULL THEN NULL ELSE lower(?17) END,?18,?19,?20,?21,?21)",
                params![binding.library_uuid,binding.root_uuid,binding.local_binding_uuid,binding.local_path,binding.canonical_path_key,binding.placement_parent,binding.placement_parent_key,binding.marker_identity,binding.filesystem_identity,binding.state,generation,binding.root_capture_epoch,binding.root_actor_epoch,binding.desired_materialized_name,binding.desired_materialized_name.to_lowercase(),binding.desired_materialization_fingerprint,binding.applied_materialized_name,binding.applied_materialization_fingerprint,revision,verification_json,now],
            )?;
        }
        transaction.execute(
            "UPDATE remote_sync_roots SET inventory_state='unselected',operation_revision=operation_revision+CASE WHEN inventory_state='unselected' THEN 0 ELSE 1 END,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3",
            params![now,binding.library_uuid,binding.root_uuid],
        )?;
        if enrollment {
            transaction.execute(
                "UPDATE root_operations SET state=CASE WHEN state='completed' THEN state ELSE 'waiting' END,phase=CASE WHEN state='completed' THEN phase ELSE 'publishing-descriptor' END,operation_revision=operation_revision+CASE WHEN state='completed' OR phase='publishing-descriptor' THEN 0 ELSE 1 END,updated_at=?1 WHERE operation_uuid=?2",
                params![now,operation_uuid],
            )?;
        } else {
            transaction.execute(
                "UPDATE root_operations SET state='completed',phase='completed',operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2",
                params![now,operation_uuid],
            )?;
        }
        transaction.commit()?;
        drop(connection);
        self.local_root_binding(&binding.library_uuid, &binding.root_uuid)?
            .context("local-root-binding-not-found")
    }

    pub fn complete_root_enrollment(
        &self,
        operation_uuid: &str,
        plan_uuid: &str,
        preflight_token: &str,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let operation: (String, String, String, String, String) = transaction
            .query_row(
                "SELECT library_uuid,root_uuid,kind,state,COALESCE(plan_uuid,'') FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .context("root-operation-not-found")?;
        if operation.2 != "enroll" || operation.4 != plan_uuid {
            bail!("stale-plan");
        }
        let binding_ready: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2 AND applied_materialized_name=desired_materialized_name AND applied_materialization_fingerprint=desired_materialization_fingerprint AND state IN ('active','paused'))",
            params![operation.0, operation.1],
            |row| row.get(0),
        )?;
        if !binding_ready {
            bail!("local-root-binding-not-ready");
        }
        transaction.execute(
            "UPDATE root_path_preflights SET consumed_at=COALESCE(consumed_at,?1) WHERE preflight_token=?2 AND library_uuid=?3",
            params![now,preflight_token,operation.0],
        )?;
        transaction.execute(
            "UPDATE root_operations SET state='completed',phase='completed',operation_revision=operation_revision+CASE WHEN state='completed' AND phase='completed' THEN 0 ELSE 1 END,error_code=NULL,error_detail=NULL,updated_at=?1 WHERE operation_uuid=?2",
            params![now,operation_uuid],
        )?;
        let completed = transaction.query_row(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],
            root_operation_record_from_row,
        )?;
        transaction.commit()?;
        Ok(completed)
    }

    #[allow(dead_code)]
    pub fn local_root_binding(
        &self,
        library_uuid: &str,
        root_uuid: &str,
    ) -> anyhow::Result<Option<LocalRootBindingRecord>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let row = connection
            .query_row(
                "SELECT library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,binding_generation,root_capture_epoch,root_actor_epoch,desired_materialized_name,desired_materialization_fingerprint,applied_materialized_name,applied_materialization_fingerprint,operation_revision,last_error_code FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
                params![library_uuid, root_uuid],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, String>(11)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, Option<String>>(14)?,
                        row.get::<_, i64>(15)?,
                        row.get::<_, Option<String>>(16)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(
                library_uuid,
                root_uuid,
                local_binding_uuid,
                local_path,
                canonical_path_key,
                placement_parent,
                placement_parent_key,
                state,
                binding_generation,
                root_capture_epoch,
                root_actor_epoch,
                desired_materialized_name,
                desired_materialization_fingerprint,
                applied_materialized_name,
                applied_materialization_fingerprint,
                operation_revision,
                last_error_code,
            )| {
                Ok(LocalRootBindingRecord {
                    library_uuid,
                    root_uuid,
                    local_binding_uuid,
                    local_path,
                    canonical_path_key,
                    placement_parent,
                    placement_parent_key,
                    state,
                    binding_generation: u64::try_from(binding_generation)
                        .map_err(|_| anyhow::anyhow!("invalid binding generation"))?,
                    root_capture_epoch,
                    root_actor_epoch,
                    desired_materialized_name,
                    desired_materialization_fingerprint,
                    applied_materialized_name,
                    applied_materialization_fingerprint,
                    operation_revision: u64::try_from(operation_revision)
                        .map_err(|_| anyhow::anyhow!("invalid root operation revision"))?,
                    last_error_code,
                })
            },
        )
        .transpose()
    }

    pub fn local_root_binding_tokens(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Vec<crate::sync::runtime::RootExecutionToken>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT root_uuid,local_binding_uuid,binding_generation FROM local_root_bindings WHERE library_uuid=?1 AND state IN ('materializing','active','paused','reconcile-required','relocating','local-rename-required') ORDER BY root_uuid",
        )?;
        let rows = statement.query_map([library_uuid], |row| {
            Ok(crate::sync::runtime::RootExecutionToken {
                library_uuid: library_uuid.to_owned(),
                root_uuid: row.get(0)?,
                local_binding_uuid: row.get(1)?,
                binding_generation: u64::try_from(row.get::<_, i64>(2)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn local_root_bindings(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Vec<LocalRootBindingRecord>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT root_uuid FROM local_root_bindings WHERE library_uuid=?1 ORDER BY root_uuid",
        )?;
        let root_uuids = statement
            .query_map([library_uuid], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        root_uuids
            .into_iter()
            .map(|root_uuid| {
                self.local_root_binding(library_uuid, &root_uuid)?
                    .ok_or_else(|| anyhow::anyhow!("local-root-binding-not-found"))
            })
            .collect()
    }

    pub(crate) fn local_root_binding_for_canonical_path(
        &self,
        canonical_path_key: &str,
    ) -> anyhow::Result<Option<LocalRootBindingRecord>> {
        let identity = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .query_row(
                "SELECT library_uuid,root_uuid FROM local_root_bindings WHERE canonical_path_key=?1",
                [canonical_path_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        identity
            .map(|(library_uuid, root_uuid)| {
                self.local_root_binding(&library_uuid, &root_uuid)?
                    .context("local-root-binding-not-found")
            })
            .transpose()
    }

    pub(crate) fn local_root_bindings_for_filesystem_identity(
        &self,
        filesystem_identity: &str,
    ) -> anyhow::Result<Vec<LocalRootBindingRecord>> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut statement = connection.prepare(
            "SELECT library_uuid,root_uuid FROM local_root_bindings WHERE filesystem_identity=?1 ORDER BY library_uuid,root_uuid",
        )?;
        let identities = statement
            .query_map([filesystem_identity], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        identities
            .into_iter()
            .map(|(library_uuid, root_uuid)| {
                self.local_root_binding(&library_uuid, &root_uuid)?
                    .context("local-root-binding-not-found")
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn require_current_root_token(
        &self,
        token: &crate::sync::runtime::RootExecutionToken,
    ) -> anyhow::Result<()> {
        let current = self
            .local_root_binding(&token.library_uuid, &token.root_uuid)?
            .ok_or_else(|| anyhow::anyhow!("stale-root-binding"))?;
        if current.local_binding_uuid != token.local_binding_uuid
            || current.binding_generation != token.binding_generation
            || !matches!(
                current.state.as_str(),
                "materializing"
                    | "active"
                    | "paused"
                    | "reconcile-required"
                    | "relocating"
                    | "local-rename-required"
            )
        {
            anyhow::bail!("stale-root-binding");
        }
        Ok(())
    }

    /// Reserves a managed-root-v2 operation journal exactly once. Reusing the
    /// operation UUID with the same immutable payload is idempotent; changing
    /// any identity, kind, or request field is rejected.
    #[allow(clippy::too_many_arguments, dead_code)]
    pub fn begin_root_operation_once(
        &self,
        operation_uuid: &str,
        library_uuid: &str,
        root_uuid: &str,
        kind: &str,
        phase: &str,
        request_hash: &str,
        request: &serde_json::Value,
        expected_operation_revision: u64,
        expected_binding_generation: Option<u64>,
    ) -> anyhow::Result<(RootOperationRecord, bool)> {
        let expected_operation_revision = i64::try_from(expected_operation_revision)
            .context("root operation revision exceeds SQLite integer range")?;
        let expected_binding_generation = expected_binding_generation
            .map(i64::try_from)
            .transpose()
            .context("binding generation exceeds SQLite integer range")?;
        let request_json = serde_json::to_string(request)?;
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                root_operation_record_from_row,
            )
            .optional()?;
        if let Some(operation) = existing {
            let stored: (String, String, String, String, String, i64, Option<i64>) = transaction
                .query_row(
                    "SELECT library_uuid,root_uuid,kind,request_hash,request_json,expected_operation_revision,expected_binding_generation FROM root_operations WHERE operation_uuid=?1",
                    [operation_uuid],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
                )?;
            if stored
                != (
                    library_uuid.to_string(),
                    root_uuid.to_string(),
                    kind.to_string(),
                    request_hash.to_string(),
                    request_json,
                    expected_operation_revision,
                    expected_binding_generation,
                )
            {
                bail!("idempotency-key-reused");
            }
            transaction.commit()?;
            return Ok((operation, false));
        }

        let root_state: (i64, Option<String>, Option<i64>, Option<i64>) = transaction
            .query_row(
                "SELECT remote.operation_revision,binding.state,binding.binding_generation,binding.operation_revision FROM remote_sync_roots remote LEFT JOIN local_root_bindings binding ON binding.library_uuid=remote.library_uuid AND binding.root_uuid=remote.root_uuid WHERE remote.library_uuid=?1 AND remote.root_uuid=?2",
                params![library_uuid, root_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?
            .context("root-not-found")?;
        let current_operation_revision = root_state.3.unwrap_or(root_state.0);
        if expected_operation_revision != current_operation_revision {
            bail!("stale-operation-revision");
        }
        if root_state.2.is_some() && expected_binding_generation.is_none() {
            bail!("stale-binding-generation");
        }
        if let Some(expected_generation) = expected_binding_generation {
            if root_state.2 != Some(expected_generation) {
                bail!("stale-binding-generation");
            }
        }
        let sealed_lineage: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM root_cutover_snapshots WHERE library_uuid=?1 AND root_uuid=?2)",
            params![library_uuid, root_uuid],
            |row| row.get(0),
        )?;
        if matches!(kind, "select" | "rebind")
            && (root_state.1.as_deref() == Some("local-only-fork") || sealed_lineage)
        {
            bail!("comparison-required");
        }
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND state IN ('pending','running','waiting'))",
            params![library_uuid, root_uuid],
            |row| row.get::<_, bool>(0),
        )? {
            bail!("operation-in-progress");
        }
        transaction.execute(
            "INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,created_at,updated_at) VALUES (?1,?2,?3,?4,'pending',?5,?6,?7,?8,?9,?10,?10)",
            params![
                operation_uuid,
                library_uuid,
                root_uuid,
                kind,
                phase,
                request_hash,
                request_json,
                expected_operation_revision,
                expected_binding_generation,
                now,
            ],
        )?;
        let operation = transaction.query_row(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],
            root_operation_record_from_row,
        )?;
        transaction.commit()?;
        Ok((operation, true))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn begin_root_relocation(
        &self,
        operation_uuid: &str,
        library_uuid: &str,
        root_uuid: &str,
        mode: &str,
        request_hash: &str,
        request: &serde_json::Value,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
        desired_materialization_fingerprint: &str,
        evidence: &crate::sync_folders::RelocationEvidence,
    ) -> anyhow::Result<(RootRelocationRecord, bool)> {
        let kind = match mode {
            "explicit-move" => "relocate",
            "automatic-rename" => "automatic-rename",
            _ => bail!("invalid relocation mode"),
        };
        let request_json = serde_json::to_string(request)?;
        let result_json = serde_json::to_string(evidence)?;
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some(existing) = transaction
            .query_row(
                "SELECT operation_uuid FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            transaction.commit()?;
            drop(connection);
            let relocation = self
                .root_relocation_operation(&existing)?
                .context("root-relocation-not-found")?;
            if relocation.operation.library_uuid != library_uuid
                || relocation.operation.root_uuid != root_uuid
                || relocation.operation.kind != kind
                || relocation.operation.request_hash != request_hash
                || relocation.operation.request_json != request_json
                || relocation.desired_materialization_fingerprint
                    != desired_materialization_fingerprint
                || relocation.source_canonical_key != evidence.source_canonical_key
                || relocation.target_canonical_key != evidence.target_canonical_key
            {
                bail!("idempotency-key-reused");
            }
            return Ok((relocation, false));
        }
        let binding: (String, String, String, i64, i64) = transaction
            .query_row(
                "SELECT local_binding_uuid,local_path,state,binding_generation,operation_revision FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
                params![library_uuid, root_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()?
            .context("local-root-binding-not-found")?;
        if !matches!(
            binding.2.as_str(),
            "active" | "paused" | "local-rename-required"
        ) {
            bail!("invalid-root-state");
        }
        if binding.3 != i64::try_from(expected_binding_generation)? {
            bail!("stale-binding-generation");
        }
        if binding.4 != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        if evidence.source_filesystem_identity.is_empty() {
            bail!("source-missing");
        }
        if crate::sync_folders::canonical_path_key(Path::new(&binding.1))
            != evidence.source_canonical_key
        {
            bail!("path-changed");
        }
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM root_operations WHERE library_uuid=?1 AND root_uuid=?2 AND state IN ('pending','running','waiting'))",
            params![library_uuid,root_uuid],
            |row| row.get::<_, bool>(0),
        )? {
            bail!("operation-in-progress");
        }
        let next_generation = expected_binding_generation.saturating_add(1);
        transaction.execute(
            "INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,result_json,expected_operation_revision,expected_binding_generation,operation_revision,created_at,updated_at) VALUES (?1,?2,?3,?4,'running','fenced',?5,?6,?7,?8,?9,1,?10,?10)",
            params![operation_uuid,library_uuid,root_uuid,kind,request_hash,request_json,result_json,i64::try_from(expected_operation_revision)?,i64::try_from(expected_binding_generation)?,now],
        )?;
        transaction.execute(
            "INSERT INTO root_relocation_operations(operation_uuid,mode,desired_materialization_fingerprint,source_path,source_canonical_key,source_filesystem_identity,target_path,target_canonical_key,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![operation_uuid,mode,desired_materialization_fingerprint,evidence.source_path,evidence.source_canonical_key,evidence.source_filesystem_identity,evidence.target_path,evidence.target_canonical_key,now],
        )?;
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET state='relocating',binding_generation=?1,operation_revision=operation_revision+1,last_error_code=NULL,last_error_detail=NULL,updated_at=?2 WHERE library_uuid=?3 AND root_uuid=?4 AND local_binding_uuid=?5 AND binding_generation=?6 AND operation_revision=?7",
            params![i64::try_from(next_generation)?,now,library_uuid,root_uuid,binding.0,i64::try_from(expected_binding_generation)?,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-binding-generation");
        }
        transaction.commit()?;
        drop(connection);
        Ok((
            self.root_relocation_operation(operation_uuid)?
                .context("root-relocation-not-found")?,
            true,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_automatic_root_rename_preflight_failure(
        &self,
        operation_uuid: &str,
        library_uuid: &str,
        root_uuid: &str,
        request_hash: &str,
        request: &serde_json::Value,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
        desired_materialization_fingerprint: &str,
        target_path: &Path,
    ) -> anyhow::Result<RootOperationRecord> {
        let request_json = serde_json::to_string(request)?;
        let target_path = target_path.to_string_lossy().into_owned();
        let target_canonical_key = crate::sync_folders::canonical_path_key(Path::new(&target_path));
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some(existing) = transaction
            .query_row(
                "SELECT operation.operation_uuid FROM root_operations operation JOIN root_relocation_operations relocation ON relocation.operation_uuid=operation.operation_uuid WHERE operation.operation_uuid=?1 AND operation.library_uuid=?2 AND operation.root_uuid=?3 AND operation.kind='automatic-rename' AND operation.request_hash=?4 AND operation.request_json=?5 AND relocation.desired_materialization_fingerprint=?6 AND relocation.target_canonical_key=?7",
                params![operation_uuid, library_uuid, root_uuid, request_hash, request_json, desired_materialization_fingerprint, target_canonical_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            let result = transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [existing],
                root_operation_record_from_row,
            )?;
            transaction.commit()?;
            return Ok(result);
        }
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM root_operations WHERE operation_uuid=?1)",
            [operation_uuid],
            |row| row.get::<_, bool>(0),
        )? {
            bail!("idempotency-key-reused");
        }
        let binding: (String, String, String, String, i64, i64) = transaction
            .query_row(
                "SELECT local_binding_uuid,local_path,canonical_path_key,state,binding_generation,operation_revision FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
                params![library_uuid, root_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .optional()?
            .context("local-root-binding-not-found")?;
        if !matches!(
            binding.3.as_str(),
            "active" | "paused" | "local-rename-required"
        ) {
            bail!("invalid-root-state");
        }
        if binding.4 != i64::try_from(expected_binding_generation)? {
            bail!("stale-binding-generation");
        }
        if binding.5 != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        if let Some(existing) = transaction
            .query_row(
                "SELECT operation.operation_uuid FROM root_operations operation JOIN root_relocation_operations relocation ON relocation.operation_uuid=operation.operation_uuid WHERE operation.library_uuid=?1 AND operation.root_uuid=?2 AND operation.kind='automatic-rename' AND operation.state='failed' AND operation.phase='local-rename-required' AND operation.error_code='local-rename-required' AND relocation.desired_materialization_fingerprint=?3 AND relocation.target_canonical_key=?4 ORDER BY operation.updated_at DESC LIMIT 1",
                params![library_uuid, root_uuid, desired_materialization_fingerprint, target_canonical_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            transaction.execute(
                "UPDATE local_root_bindings SET state='local-rename-required',last_error_code='local-rename-required',last_error_detail=NULL,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3 AND local_binding_uuid=?4 AND binding_generation=?5 AND operation_revision=?6",
                params![now, library_uuid, root_uuid, binding.0, binding.4, binding.5],
            )?;
            let result = transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [existing],
                root_operation_record_from_row,
            )?;
            transaction.commit()?;
            return Ok(result);
        }
        let source_filesystem_identity =
            crate::library_path::local_file_identity(Path::new(&binding.1)).ok();
        let result_json = serde_json::json!({"preflightErrorCode":"local-rename-required"});
        transaction.execute(
            "INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,result_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at) VALUES (?1,?2,?3,'automatic-rename','failed','local-rename-required',?4,?5,?6,?7,?8,1,0,'local-rename-required',?9,?9)",
            params![operation_uuid, library_uuid, root_uuid, request_hash, request_json, serde_json::to_string(&result_json)?, i64::try_from(expected_operation_revision)?, i64::try_from(expected_binding_generation)?, now],
        )?;
        transaction.execute(
            "INSERT INTO root_relocation_operations(operation_uuid,mode,desired_materialization_fingerprint,source_path,source_canonical_key,source_filesystem_identity,target_path,target_canonical_key,cleanup_state,updated_at) VALUES (?1,'automatic-rename',?2,?3,?4,?5,?6,?7,'not-required',?8)",
            params![operation_uuid, desired_materialization_fingerprint, binding.1, binding.2, source_filesystem_identity, target_path, target_canonical_key, now],
        )?;
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET state='local-rename-required',operation_revision=operation_revision+1,last_error_code='local-rename-required',last_error_detail=NULL,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3 AND local_binding_uuid=?4 AND binding_generation=?5 AND operation_revision=?6",
            params![now, library_uuid, root_uuid, binding.0, binding.4, binding.5],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        let result = transaction.query_row(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],
            root_operation_record_from_row,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn advance_root_relocation(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        phase: &str,
        staging_path: Option<&str>,
        target_filesystem_identity: Option<&str>,
        verification_digest: Option<&str>,
        transferred_bytes: u64,
        verified_bytes: u64,
        cutover_committed: bool,
    ) -> anyhow::Result<RootRelocationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let state = if phase == "completed" {
            "completed"
        } else {
            "running"
        };
        let changed = transaction.execute(
            "UPDATE root_operations SET state=?1,phase=?2,operation_revision=operation_revision+1,cutover_committed=MAX(cutover_committed,?3),error_code=NULL,error_detail=NULL,updated_at=?4 WHERE operation_uuid=?5 AND kind IN ('relocate','automatic-rename') AND state IN ('pending','running','waiting') AND operation_revision=?6 AND EXISTS(SELECT 1 FROM local_root_bindings binding WHERE binding.library_uuid=root_operations.library_uuid AND binding.root_uuid=root_operations.root_uuid AND binding.local_binding_uuid=COALESCE(json_extract(root_operations.request_json,'$.localBindingUuid'),binding.local_binding_uuid) AND binding.binding_generation=root_operations.expected_binding_generation+1)",
            params![state,phase,cutover_committed as i64,now,operation_uuid,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            let completed: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM root_operations WHERE operation_uuid=?1 AND state='completed')",
                [operation_uuid],
                |row| row.get(0),
            )?;
            if completed {
                transaction.commit()?;
                drop(connection);
                return self
                    .root_relocation_operation(operation_uuid)?
                    .context("root-relocation-not-found");
            }
            bail!("stale-operation-revision");
        }
        transaction.execute(
            "UPDATE root_relocation_operations SET staging_path=COALESCE(?1,staging_path),target_filesystem_identity=COALESCE(?2,target_filesystem_identity),verification_digest=COALESCE(?3,verification_digest),transferred_bytes=MAX(transferred_bytes,?4),verified_bytes=MAX(verified_bytes,?5),cleanup_state=CASE WHEN ?6='cleanup' THEN 'ready' WHEN ?6='completed' THEN 'complete' ELSE cleanup_state END,updated_at=?7 WHERE operation_uuid=?8",
            params![staging_path,target_filesystem_identity,verification_digest,i64::try_from(transferred_bytes)?,i64::try_from(verified_bytes)?,phase,now,operation_uuid],
        )?;
        transaction.commit()?;
        drop(connection);
        self.root_relocation_operation(operation_uuid)?
            .context("root-relocation-not-found")
    }

    pub fn record_root_relocation_transfer_progress(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        staging_path: &str,
        transferred_bytes: u64,
        checkpoint: &crate::sync_folders::RelocationTransferCheckpoint,
    ) -> anyhow::Result<RootRelocationRecord> {
        let now = unix_timestamp_millis();
        let checkpoint = serde_json::to_string(checkpoint)?;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE root_operations SET operation_revision=operation_revision+1,result_json=json_set(COALESCE(result_json,'{}'),'$.transferProgress',json(?1),'$.transferProgress.updatedAt',?2),updated_at=?2 WHERE operation_uuid=?3 AND kind IN ('relocate','automatic-rename') AND state='running' AND phase='transferring' AND operation_revision=?4",
            params![checkpoint,now,operation_uuid,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        transaction.execute(
            "UPDATE root_relocation_operations SET staging_path=?1,transferred_bytes=MAX(transferred_bytes,?2),updated_at=?3 WHERE operation_uuid=?4",
            params![staging_path,i64::try_from(transferred_bytes)?,now,operation_uuid],
        )?;
        transaction.commit()?;
        drop(connection);
        self.root_relocation_operation(operation_uuid)?
            .context("root-relocation-not-found")
    }

    pub fn root_relocation_transfer_checkpoint(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<crate::sync_folders::RelocationTransferCheckpoint> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let value: Option<String> = connection
            .query_row(
                "SELECT json_extract(result_json,'$.transferProgress') FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(Into::into)
            .map(|value| value.unwrap_or_default())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_root_relocation_host_cutover(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        local_binding_uuid: &str,
        binding_generation: u64,
        target_path: &str,
        target_canonical_key: &str,
        placement_parent: &str,
        placement_parent_key: &str,
        marker_identity: &str,
        filesystem_identity: &str,
        applied_materialized_name: &str,
        applied_materialization_fingerprint: &str,
    ) -> anyhow::Result<RootRelocationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let operation: (String, String, String, String, i64, String, i64) = transaction.query_row(
            "SELECT operation.library_uuid,operation.root_uuid,operation.state,operation.phase,operation.cutover_committed,relocation.desired_materialization_fingerprint,operation.operation_revision FROM root_operations operation JOIN root_relocation_operations relocation ON relocation.operation_uuid=operation.operation_uuid WHERE operation.operation_uuid=?1 AND operation.kind IN ('relocate','automatic-rename')",
            [operation_uuid],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
        ).context("root-operation-not-found")?;
        if operation.2 == "completed" {
            transaction.commit()?;
            drop(connection);
            return self
                .root_relocation_operation(operation_uuid)?
                .context("root-relocation-not-found");
        }
        if operation.3 == "reconciling" && operation.4 != 0 {
            let already: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND canonical_path_key=?5 AND state IN ('reconcile-required','active','paused'))",
                params![operation.0,operation.1,local_binding_uuid,i64::try_from(binding_generation)?,target_canonical_key],
                |row| row.get(0),
            )?;
            if !already {
                bail!("stale-binding-generation");
            }
            transaction.commit()?;
            drop(connection);
            return self
                .root_relocation_operation(operation_uuid)?
                .context("root-relocation-not-found");
        }
        if operation.3 != "host-cutover"
            || operation.6 != i64::try_from(expected_operation_revision)?
        {
            bail!("stale-operation-revision");
        }
        if operation.5 != applied_materialization_fingerprint {
            bail!("stale-operation-revision");
        }
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET local_path=?1,canonical_path_key=?2,placement_parent=?3,placement_parent_key=?4,marker_identity=?5,filesystem_identity=?6,state='reconcile-required',applied_materialized_name=CASE WHEN desired_materialization_fingerprint=?9 THEN ?7 ELSE applied_materialized_name END,applied_materialized_name_key=CASE WHEN desired_materialization_fingerprint=?9 THEN ?8 ELSE applied_materialized_name_key END,applied_materialization_fingerprint=CASE WHEN desired_materialization_fingerprint=?9 THEN ?9 ELSE applied_materialization_fingerprint END,operation_revision=operation_revision+1,last_error_code=NULL,last_error_detail=NULL,updated_at=?10 WHERE library_uuid=?11 AND root_uuid=?12 AND local_binding_uuid=?13 AND binding_generation=?14 AND state IN ('relocating','reconcile-required')",
            params![target_path,target_canonical_key,placement_parent,placement_parent_key,marker_identity,filesystem_identity,applied_materialized_name,applied_materialized_name.to_lowercase(),applied_materialization_fingerprint,now,operation.0,operation.1,local_binding_uuid,i64::try_from(binding_generation)?],
        )?;
        if changed != 1 {
            let already: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND canonical_path_key=?5 AND state IN ('reconcile-required','active'))",
                params![operation.0,operation.1,local_binding_uuid,i64::try_from(binding_generation)?,target_canonical_key],
                |row| row.get(0),
            )?;
            if !already {
                bail!("stale-binding-generation");
            }
        }
        let changed = transaction.execute(
            "UPDATE root_operations SET state='running',phase='reconciling',cutover_committed=1,operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2 AND state='running' AND phase='host-cutover' AND operation_revision=?3",
            params![now,operation_uuid,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        transaction.execute(
            "UPDATE root_relocation_operations SET target_filesystem_identity=?1,updated_at=?2 WHERE operation_uuid=?3",
            params![filesystem_identity,now,operation_uuid],
        )?;
        transaction.commit()?;
        drop(connection);
        self.root_relocation_operation(operation_uuid)?
            .context("root-relocation-not-found")
    }

    pub fn complete_root_relocation(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        binding_generation: u64,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let identity: (String, String, String, String, String, i64) = transaction.query_row(
            "SELECT library_uuid,root_uuid,request_json,state,phase,operation_revision FROM root_operations WHERE operation_uuid=?1 AND kind IN ('relocate','automatic-rename')",
            [operation_uuid],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        ).context("root-operation-not-found")?;
        if identity.3 == "completed" {
            let result = transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],root_operation_record_from_row,
            )?;
            transaction.commit()?;
            return Ok(result);
        }
        if identity.3 != "running"
            || identity.4 != "cleanup"
            || identity.5 != i64::try_from(expected_operation_revision)?
        {
            bail!("stale-operation-revision");
        }
        let resume_state = relocation_resume_state(&identity.2);
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET state=?1,operation_revision=operation_revision+1,last_error_code=NULL,last_error_detail=NULL,updated_at=?2 WHERE library_uuid=?3 AND root_uuid=?4 AND binding_generation=?5 AND state IN ('reconcile-required','active','paused')",
            params![resume_state,now,identity.0,identity.1,i64::try_from(binding_generation)?],
        )?;
        if changed != 1 {
            bail!("stale-binding-generation");
        }
        let changed = transaction.execute(
            "UPDATE root_operations SET state='completed',phase='completed',cutover_committed=1,operation_revision=operation_revision+1,error_code=NULL,error_detail=NULL,updated_at=?1 WHERE operation_uuid=?2 AND state='running' AND phase='cleanup' AND operation_revision=?3",
            params![now,operation_uuid,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        transaction.execute(
            "UPDATE root_relocation_operations SET cleanup_state='complete',updated_at=?1 WHERE operation_uuid=?2",
            params![now,operation_uuid],
        )?;
        let result = transaction.query_row(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],root_operation_record_from_row,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn fail_root_relocation_cleanup(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        binding_generation: u64,
        error_code: &str,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let identity: (String, String, String, i64) = transaction.query_row(
            "SELECT library_uuid,root_uuid,request_json,operation_revision FROM root_operations WHERE operation_uuid=?1 AND kind IN ('relocate','automatic-rename') AND state='running' AND phase='cleanup'",
            [operation_uuid],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).context("stale-operation-revision")?;
        if identity.3 != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        let resume_state = relocation_resume_state(&identity.2);
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET state=?1,operation_revision=operation_revision+1,last_error_code=?2,last_error_detail=NULL,updated_at=?3 WHERE library_uuid=?4 AND root_uuid=?5 AND binding_generation=?6 AND state IN ('reconcile-required','active','paused')",
            params![resume_state,error_code,now,identity.0,identity.1,i64::try_from(binding_generation)?],
        )?;
        if changed != 1 {
            bail!("stale-binding-generation");
        }
        let changed = transaction.execute(
            "UPDATE root_operations SET state='failed',phase='transfer-failed',error_code=?1,error_detail=NULL,operation_revision=operation_revision+1,updated_at=?2 WHERE operation_uuid=?3 AND operation_revision=?4 AND state='running' AND phase='cleanup'",
            params![error_code,now,operation_uuid,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        transaction.execute(
            "UPDATE root_relocation_operations SET cleanup_state='failed',updated_at=?1 WHERE operation_uuid=?2",
            params![now,operation_uuid],
        )?;
        let result = transaction.query_row(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],root_operation_record_from_row,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn repair_root_relocation_after_catalog_cutover(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
    ) -> anyhow::Result<RootRelocationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let operation: (String, String, i64, String) = transaction.query_row(
            "SELECT library_uuid,root_uuid,expected_binding_generation,request_json FROM root_operations WHERE operation_uuid=?1 AND kind IN ('relocate','automatic-rename') AND state='failed' AND cutover_committed=0 AND operation_revision=?2",
            params![operation_uuid, i64::try_from(expected_operation_revision)?],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).context("stale-operation-revision")?;
        let local_binding_uuid =
            relocation_local_binding_uuid(&operation.3).context("stale-binding-generation")?;
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET state='relocating',operation_revision=operation_revision+1,last_error_code=NULL,last_error_detail=NULL,updated_at=?1 WHERE library_uuid=?2 AND root_uuid=?3 AND local_binding_uuid=?4 AND binding_generation=?5 AND state IN ('active','paused','local-rename-required','relocating')",
            params![now,operation.0,operation.1,local_binding_uuid,operation.2.saturating_add(1)],
        )?;
        if changed != 1 {
            bail!("stale-binding-generation");
        }
        let changed = transaction.execute(
            "UPDATE root_operations SET state='running',phase='host-cutover',cutover_committed=1,operation_revision=operation_revision+1,error_code=NULL,error_detail=NULL,updated_at=?1 WHERE operation_uuid=?2 AND state='failed' AND operation_revision=?3",
            params![now, operation_uuid, i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        transaction.commit()?;
        drop(connection);
        self.root_relocation_operation(operation_uuid)?
            .context("root-relocation-not-found")
    }

    pub fn complete_logical_root_rename(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let changed = connection.execute(
            "UPDATE root_operations SET state='completed',phase='completed',operation_revision=operation_revision+1,updated_at=?1 WHERE operation_uuid=?2 AND kind='logical-rename' AND state IN ('pending','running','waiting')",
            params![now,operation_uuid],
        )?;
        if changed != 1 {
            let completed: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM root_operations WHERE operation_uuid=?1 AND kind='logical-rename' AND state='completed')",
                [operation_uuid],|row|row.get(0),
            )?;
            if !completed {
                bail!("root-operation-not-found");
            }
        }
        drop(connection);
        self.root_operation(operation_uuid)?
            .context("root-operation-not-found")
    }

    pub fn fail_root_relocation(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        phase: &str,
        error_code: &str,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let identity: (String, String, String, String, i64, String, i64, i64) = transaction.query_row(
            "SELECT library_uuid,root_uuid,kind,state,cutover_committed,request_json,operation_revision,expected_binding_generation FROM root_operations WHERE operation_uuid=?1 AND kind IN ('relocate','automatic-rename')",
            [operation_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
        ).context("root-operation-not-found")?;
        if identity.3 == "failed" {
            let result = transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],root_operation_record_from_row,
            )?;
            transaction.commit()?;
            return Ok(result);
        }
        if !matches!(identity.3.as_str(), "pending" | "running" | "waiting")
            || identity.6 != i64::try_from(expected_operation_revision)?
        {
            bail!("stale-operation-revision");
        }
        let binding_state = if identity.4 != 0 {
            "reconcile-required"
        } else if identity.2 == "automatic-rename" {
            "local-rename-required"
        } else {
            relocation_resume_state(&identity.5)
        };
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET state=?1,last_error_code=?2,last_error_detail=NULL,operation_revision=operation_revision+1,updated_at=?3 WHERE library_uuid=?4 AND root_uuid=?5 AND binding_generation=?6",
            params![binding_state,error_code,now,identity.0,identity.1,identity.7.saturating_add(1)],
        )?;
        if changed != 1 {
            bail!("stale-binding-generation");
        }
        let changed = transaction.execute(
            "UPDATE root_operations SET state='failed',phase=?1,error_code=?2,error_detail=NULL,operation_revision=operation_revision+1,updated_at=?3 WHERE operation_uuid=?4 AND operation_revision=?5 AND state IN ('pending','running','waiting')",
            params![phase,error_code,now,operation_uuid,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        let result = transaction.query_row(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],root_operation_record_from_row,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn cancel_root_relocation(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
    ) -> anyhow::Result<RootOperationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let identity: (String, String, String, i64, i64, String) = transaction.query_row(
            "SELECT library_uuid,root_uuid,state,operation_revision,cutover_committed,request_json FROM root_operations WHERE operation_uuid=?1 AND kind IN ('relocate','automatic-rename')",
            [operation_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        ).context("root-operation-not-found")?;
        if identity.4 != 0 {
            bail!("cutover-committed");
        }
        let binding_generation: i64 = transaction.query_row(
            "SELECT binding_generation FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
            params![identity.0,identity.1],|row|row.get(0),
        )?;
        if binding_generation != i64::try_from(expected_binding_generation)? {
            bail!("stale-binding-generation");
        }
        if let Some(expected_local_binding_uuid) = relocation_local_binding_uuid(&identity.5) {
            let current_local_binding_uuid: String = transaction.query_row(
                "SELECT local_binding_uuid FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
                params![identity.0, identity.1],
                |row| row.get(0),
            )?;
            if current_local_binding_uuid != expected_local_binding_uuid {
                bail!("stale-binding-generation");
            }
        }
        if identity.2 == "cancelled"
            && identity.3 == i64::try_from(expected_operation_revision.saturating_add(1))?
        {
            let result = transaction.query_row(
                "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
                [operation_uuid],root_operation_record_from_row,
            )?;
            transaction.commit()?;
            return Ok(result);
        }
        if !matches!(
            identity.2.as_str(),
            "pending" | "running" | "waiting" | "failed"
        ) {
            bail!("invalid-root-state");
        }
        if identity.3 != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        let resume_state = relocation_resume_state(&identity.5);
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET state=?1,operation_revision=operation_revision+1,last_error_code=NULL,last_error_detail=NULL,updated_at=?2 WHERE library_uuid=?3 AND root_uuid=?4 AND binding_generation=?5 AND state IN ('relocating','active','paused','local-rename-required')",
            params![resume_state,now,identity.0,identity.1,i64::try_from(expected_binding_generation)?],
        )?;
        if changed != 1 {
            bail!("stale-binding-generation");
        }
        let changed = transaction.execute(
            "UPDATE root_operations SET state='cancelled',phase='cancelled',operation_revision=operation_revision+1,error_code=NULL,error_detail=NULL,updated_at=?1 WHERE operation_uuid=?2 AND operation_revision=?3 AND state IN ('pending','running','waiting','failed')",
            params![now,operation_uuid,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        let result = transaction.query_row(
            "SELECT operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,error_code,created_at,updated_at FROM root_operations WHERE operation_uuid=?1",
            [operation_uuid],root_operation_record_from_row,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn retry_root_relocation(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
    ) -> anyhow::Result<RootRelocationRecord> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let operation: (String, String, String, String, i64, i64, String, Option<String>, String, i64) = transaction.query_row(
            "SELECT operation.library_uuid,operation.root_uuid,operation.kind,operation.state,operation.operation_revision,operation.cutover_committed,relocation.desired_materialization_fingerprint,relocation.source_filesystem_identity,operation.request_json,operation.expected_binding_generation FROM root_operations operation JOIN root_relocation_operations relocation ON relocation.operation_uuid=operation.operation_uuid WHERE operation.operation_uuid=?1 AND operation.kind IN ('relocate','automatic-rename')",
            [operation_uuid],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?)),
        ).context("root-operation-not-found")?;
        let binding: (String, i64, String, String) = transaction.query_row(
            "SELECT local_binding_uuid,binding_generation,state,desired_materialization_fingerprint FROM local_root_bindings WHERE library_uuid=?1 AND root_uuid=?2",
            params![operation.0,operation.1],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).context("local-root-binding-not-found")?;
        if binding.1 != i64::try_from(expected_binding_generation)? {
            bail!("stale-binding-generation");
        }
        if relocation_local_binding_uuid(&operation.8)
            .as_deref()
            .is_some_and(|expected| expected != binding.0)
        {
            bail!("stale-binding-generation");
        }
        if operation.2 == "automatic-rename" && binding.3 != operation.6 {
            bail!("stale-operation-revision");
        }
        if operation.7.as_deref().is_none_or(str::is_empty) {
            bail!("stale-plan");
        }
        if serde_json::from_str::<serde_json::Value>(&operation.8)
            .ok()
            .and_then(|request| request.get("pathEvidence").cloned())
            .is_none()
        {
            bail!("stale-plan");
        }
        if operation.3 == "running"
            && operation.4 == i64::try_from(expected_operation_revision.saturating_add(1))?
        {
            transaction.commit()?;
            drop(connection);
            return self
                .root_relocation_operation(operation_uuid)?
                .context("root-relocation-not-found");
        }
        if operation.3 != "failed" {
            bail!("invalid-root-state");
        }
        if operation.4 != i64::try_from(expected_operation_revision)? {
            bail!("stale-operation-revision");
        }
        let next_binding_state = if operation.5 != 0 {
            "reconcile-required"
        } else {
            "relocating"
        };
        let next_binding_generation = if operation.5 == 0 && binding.1 == operation.9 {
            operation.9.saturating_add(1)
        } else {
            binding.1
        };
        let changed = transaction.execute(
            "UPDATE local_root_bindings SET state=?1,binding_generation=?2,last_error_code=NULL,last_error_detail=NULL,operation_revision=operation_revision+1,updated_at=?3 WHERE library_uuid=?4 AND root_uuid=?5 AND local_binding_uuid=?6 AND binding_generation=?7 AND state IN ('active','paused','local-rename-required','relocating','reconcile-required')",
            params![next_binding_state,next_binding_generation,now,operation.0,operation.1,binding.0,i64::try_from(expected_binding_generation)?],
        )?;
        if changed != 1 {
            bail!("stale-binding-generation");
        }
        let changed = transaction.execute(
            "UPDATE root_operations SET state='running',phase=CASE WHEN cutover_committed=1 THEN 'reconciling' ELSE 'fenced' END,operation_revision=operation_revision+1,error_code=NULL,error_detail=NULL,updated_at=?1 WHERE operation_uuid=?2 AND state='failed' AND operation_revision=?3",
            params![now,operation_uuid,i64::try_from(expected_operation_revision)?],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        transaction.commit()?;
        drop(connection);
        self.root_relocation_operation(operation_uuid)?
            .context("root-relocation-not-found")
    }

    pub fn mark_root_relocation_projection_repaired(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let changed = connection.execute(
            "UPDATE root_relocation_operations SET cleanup_state='not-required',updated_at=?1 WHERE operation_uuid=?2 AND EXISTS(SELECT 1 FROM root_operations operation WHERE operation.operation_uuid=root_relocation_operations.operation_uuid AND operation.state IN ('cancelled','failed') AND operation.cutover_committed=0)",
            params![unix_timestamp_millis(), operation_uuid],
        )?;
        if changed != 1 {
            bail!("stale-operation-revision");
        }
        Ok(())
    }

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
            .any(|capability| capability == crate::plugins::SYNC_PROVIDER_V1_CAPABILITY)
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
        connection
            .query_row(
                "SELECT account_uuid,secret_ref,generation FROM plugin_accounts WHERE plugin_id=?1",
                [plugin_id],
                |row| {
                    Ok(PluginAccountSecret {
                        account_uuid: row.get(0)?,
                        secret_ref: row.get(1)?,
                        generation: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn update_local_root_marker_identity(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        local_binding_uuid: &str,
        binding_generation: u64,
        marker_identity: &str,
    ) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let changed = connection.execute(
            "UPDATE local_root_bindings SET marker_identity=?1,updated_at=?2 WHERE library_uuid=?3 AND root_uuid=?4 AND local_binding_uuid=?5 AND binding_generation=?6",
            params![
                marker_identity,
                unix_timestamp_millis(),
                library_uuid,
                root_uuid,
                local_binding_uuid,
                i64::try_from(binding_generation)?,
            ],
        )?;
        if changed != 1 {
            bail!("stale-binding-generation");
        }
        Ok(())
    }

    #[cfg(test)]
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
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_auth_flow_ownership WHERE plugin_id=?1 AND state='cancelling')",
            [plugin_id],
            |row| row.get::<_, bool>(0),
        )? {
            return Ok(false);
        }
        let replaced: Option<(String, String)> = transaction.query_row(
            "SELECT account_uuid,secret_ref FROM plugin_accounts WHERE plugin_id=?1 AND secret_ref IS NOT NULL AND secret_ref IS NOT ?2",
            params![plugin_id, secret_ref],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        if let Some((account_uuid, old_secret_ref)) =
            replaced.filter(|_| plugin_id == ONEDRIVE_PLUGIN_ID)
        {
            let now = unix_timestamp_millis();
            enqueue_onedrive_secret_retirements(
                &transaction,
                plugin_id,
                &account_uuid,
                Some(&old_secret_ref),
                now,
            )?;
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
        transaction.execute(
            "UPDATE plugin_auth_flow_ownership SET state='cancelling' WHERE plugin_id=?1",
            [plugin_id],
        )?;
        let retiring: Option<(String, String)> = transaction
            .query_row(
                "SELECT account_uuid,secret_ref FROM plugin_accounts WHERE plugin_id=?1 AND generation=?2 AND secret_ref IS NOT NULL",
                params![plugin_id,expected_generation],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((account_uuid, secret_ref)) =
            retiring.filter(|_| plugin_id == ONEDRIVE_PLUGIN_ID)
        {
            enqueue_onedrive_secret_retirements(
                &transaction,
                plugin_id,
                &account_uuid,
                Some(&secret_ref),
                unix_timestamp_millis(),
            )?;
        }
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

    pub fn onedrive_configuration(&self) -> anyhow::Result<OneDriveConfiguration> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let (host_override, revision): (Option<String>, i64) = connection.query_row(
            "SELECT host_override_client_id,revision FROM onedrive_configuration WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let environment =
            normalized_client_id(std::env::var("MEGLE_ONEDRIVE_CLIENT_ID").ok().as_deref());
        let (source, configured, error_code) = match environment {
            Ok(Some(_)) => ("environment", true, None),
            Err(_) => (
                "environment",
                false,
                Some("invalid-onedrive-client-id".to_owned()),
            ),
            Ok(None) => match normalized_client_id(host_override.as_deref()) {
                Ok(Some(_)) => ("host-override", true, None),
                Err(_) => (
                    "host-override",
                    false,
                    Some("invalid-onedrive-client-id".to_owned()),
                ),
                Ok(None) => (
                    "missing",
                    false,
                    Some("onedrive-client-id-missing".to_owned()),
                ),
            },
        };
        Ok(OneDriveConfiguration {
            source: source.into(),
            configured,
            read_only: source == "environment",
            revision: revision as u64,
            error_code,
        })
    }

    fn effective_onedrive_client_id_for_override(
        &self,
        host_override: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        if let Some(value) =
            normalized_client_id(std::env::var("MEGLE_ONEDRIVE_CLIENT_ID").ok().as_deref())?
        {
            return Ok(Some(value));
        }
        if let Some(value) = normalized_client_id(host_override)? {
            return Ok(Some(value));
        }
        Ok(None)
    }

    pub fn effective_onedrive_client_id(&self) -> anyhow::Result<Option<String>> {
        let host_override: Option<String> = self
            .connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT host_override_client_id FROM onedrive_configuration WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
        self.effective_onedrive_client_id_for_override(host_override.as_deref())
    }

    pub fn plan_onedrive_configuration(
        &self,
        requested_override: Option<&str>,
        expected_revision: u64,
    ) -> anyhow::Result<OneDriveConfigurationPlan> {
        if std::env::var("MEGLE_ONEDRIVE_CLIENT_ID")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
        {
            anyhow::bail!("onedrive-configuration-environment-locked");
        }
        let requested = normalized_client_id(requested_override)?;
        let now = unix_timestamp_millis();
        let expires_at = now + 5 * 60 * 1000;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let (current, revision): (Option<String>, i64) = transaction.query_row(
            "SELECT host_override_client_id,revision FROM onedrive_configuration WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if revision as u64 != expected_revision {
            anyhow::bail!("onedrive-configuration-revision-conflict");
        }
        let current_effective =
            self.effective_onedrive_client_id_for_override(current.as_deref())?;
        let requested_effective =
            self.effective_onedrive_client_id_for_override(requested.as_deref())?;
        let effective_changed = current_effective != requested_effective;
        let affected_libraries: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM library_sync_bindings WHERE plugin_id=?1",
            [ONEDRIVE_PLUGIN_ID],
            |row| row.get(0),
        )?;
        let authenticated: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_accounts WHERE plugin_id=?1 AND auth_state='authenticated')", [ONEDRIVE_PLUGIN_ID], |row| row.get(0)
        )?;
        let plan_uuid = uuid::Uuid::now_v7().to_string();
        let requires_confirmation = effective_changed && (authenticated || affected_libraries > 0);
        transaction.execute(
            "INSERT INTO onedrive_configuration_plans(plan_uuid,expected_revision,requested_override,effective_changed,affected_libraries,requires_confirmation,expires_at,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![plan_uuid,revision,requested,effective_changed,affected_libraries,requires_confirmation,expires_at,now],
        )?;
        transaction.commit()?;
        Ok(OneDriveConfigurationPlan {
            plan_uuid,
            expected_revision,
            effective_changed,
            affected_libraries: affected_libraries as u64,
            requires_confirmation,
            expires_at,
        })
    }

    pub fn commit_onedrive_configuration(
        &self,
        plan_uuid: &str,
        expected_revision: u64,
        confirmed: bool,
    ) -> anyhow::Result<OneDriveConfigurationCommit> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let (plan_revision, requested, changed, requires_confirmation, expires_at): (i64,Option<String>,bool,bool,i64) = transaction.query_row(
            "SELECT expected_revision,requested_override,effective_changed,requires_confirmation,expires_at FROM onedrive_configuration_plans WHERE plan_uuid=?1", [plan_uuid],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).optional()?.ok_or_else(|| anyhow::anyhow!("onedrive-configuration-plan-invalid"))?;
        if expires_at < now {
            anyhow::bail!("onedrive-configuration-plan-expired");
        }
        if plan_revision as u64 != expected_revision {
            anyhow::bail!("onedrive-configuration-revision-conflict");
        }
        if requires_confirmation && !confirmed {
            anyhow::bail!("onedrive-configuration-confirmation-required");
        }
        let current_revision: i64 = transaction.query_row(
            "SELECT revision FROM onedrive_configuration WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if current_revision as u64 != expected_revision {
            anyhow::bail!("onedrive-configuration-revision-conflict");
        }
        let fences = if changed {
            transaction.execute(
                "UPDATE plugin_auth_flow_ownership SET state='cancelling' WHERE plugin_id=?1",
                [ONEDRIVE_PLUGIN_ID],
            )?;
            let fences = pause_plugin_bindings_in_transaction(&transaction, ONEDRIVE_PLUGIN_ID)?;
            let retiring: Option<(String, String)> = transaction
                .query_row(
                    "SELECT account_uuid,secret_ref FROM plugin_accounts WHERE plugin_id=?1 AND secret_ref IS NOT NULL",
                    [ONEDRIVE_PLUGIN_ID],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((account_uuid, secret_ref)) = retiring {
                enqueue_onedrive_secret_retirements(
                    &transaction,
                    ONEDRIVE_PLUGIN_ID,
                    &account_uuid,
                    Some(&secret_ref),
                    now,
                )?;
            }
            transaction.execute("UPDATE onedrive_configuration SET host_override_client_id=?1,revision=revision+1,updated_at=?2 WHERE singleton=1", params![requested,now])?;
            transaction.execute("UPDATE plugin_accounts SET secret_ref=NULL,stable_account_identity=NULL,account_kind=NULL,auth_state='authorization-required',generation=generation+1,updated_at=?1 WHERE plugin_id=?2", params![now,ONEDRIVE_PLUGIN_ID])?;
            transaction.execute("UPDATE plugin_runtime_state SET state='starting',pid=NULL,session_id=NULL,last_error_code=NULL,last_error_detail=NULL,updated_at=?1 WHERE plugin_id=?2", params![now,ONEDRIVE_PLUGIN_ID])?;
            fences
        } else {
            Vec::new()
        };
        transaction.execute(
            "DELETE FROM onedrive_configuration_plans WHERE plan_uuid=?1",
            [plan_uuid],
        )?;
        transaction.commit()?;
        Ok(OneDriveConfigurationCommit {
            fences,
            effective_changed: changed,
        })
    }

    pub fn due_credential_retirements(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<CredentialRetirement>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT retirement_uuid,plugin_id,account_uuid,secret_ref,namespace,purpose,attempts FROM plugin_credential_retirements WHERE next_retry_at<=?1 ORDER BY created_at LIMIT ?2",
        )?;
        let retirements = statement
            .query_map(params![unix_timestamp_millis(), limit as i64], |row| {
                Ok(CredentialRetirement {
                    retirement_uuid: row.get(0)?,
                    plugin_id: row.get(1)?,
                    account_uuid: row.get(2)?,
                    secret_ref: row.get(3)?,
                    namespace: row.get(4)?,
                    purpose: row.get(5)?,
                    attempts: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(retirements)
    }

    pub fn complete_credential_retirement(&self, retirement_uuid: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let retirement: Option<(String, String, String)> = transaction
            .query_row(
                "SELECT plugin_id,account_uuid,secret_ref FROM plugin_credential_retirements WHERE retirement_uuid=?1",
                [retirement_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        transaction.execute(
            "DELETE FROM plugin_credential_retirements WHERE retirement_uuid=?1",
            [retirement_uuid],
        )?;
        if let Some((plugin_id, account_uuid, secret_ref)) = retirement {
            transaction.execute("DELETE FROM plugin_secret_handles WHERE plugin_id=?1 AND account_uuid=?2 AND secret_ref=?3", params![plugin_id, account_uuid, secret_ref])?;
            transaction.execute(
                "UPDATE plugin_runtime_state SET state='starting',last_error_code=NULL,last_error_detail=NULL,updated_at=?1 WHERE plugin_id=?2 AND state='degraded' AND last_error_code='credential-retirement-pending' AND NOT EXISTS(SELECT 1 FROM plugin_credential_retirements q WHERE q.plugin_id=?2)",
                params![unix_timestamp_millis(), plugin_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn reserve_plugin_secret_handle_for_launch(
        &self,
        plugin_id: &str,
        account_uuid: &str,
        expected_fingerprint: &str,
        secret_ref: &str,
        namespace: &str,
        purpose: &str,
    ) -> anyhow::Result<bool> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if !plugin_runtime_launch_matches(&transaction, plugin_id, expected_fingerprint)? {
            return Ok(false);
        }
        let account_matches: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_accounts WHERE plugin_id=?1 AND account_uuid=?2)",
            params![plugin_id, account_uuid],
            |row| row.get(0),
        )?;
        if !account_matches {
            return Ok(false);
        }
        let inserted = transaction.execute("INSERT OR IGNORE INTO plugin_secret_creations(plugin_id,account_uuid,secret_ref,namespace,purpose,launch_fingerprint,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![plugin_id,account_uuid,secret_ref,namespace,purpose,expected_fingerprint,unix_timestamp_millis()])?;
        transaction.commit()?;
        Ok(inserted == 1)
    }

    #[cfg(test)]
    pub fn register_plugin_secret_handle_for_launch(
        &self,
        plugin_id: &str,
        account_uuid: &str,
        expected_fingerprint: &str,
        secret_ref: &str,
        namespace: &str,
        purpose: &str,
    ) -> anyhow::Result<bool> {
        if !self.reserve_plugin_secret_handle_for_launch(
            plugin_id,
            account_uuid,
            expected_fingerprint,
            secret_ref,
            namespace,
            purpose,
        )? {
            return Ok(false);
        }
        self.finalize_plugin_secret_handle_for_launch(
            plugin_id,
            account_uuid,
            expected_fingerprint,
            secret_ref,
        )
    }

    pub fn finalize_plugin_secret_handle_for_launch(
        &self,
        plugin_id: &str,
        account_uuid: &str,
        expected_fingerprint: &str,
        secret_ref: &str,
    ) -> anyhow::Result<bool> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if !plugin_runtime_launch_matches(&transaction, plugin_id, expected_fingerprint)? {
            return Ok(false);
        }
        let pending: Option<(String, String)> = transaction.query_row(
            "SELECT namespace,purpose FROM plugin_secret_creations WHERE plugin_id=?1 AND account_uuid=?2 AND secret_ref=?3 AND launch_fingerprint=?4",
            params![plugin_id, account_uuid, secret_ref, expected_fingerprint],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let Some((namespace, purpose)) = pending else {
            return Ok(false);
        };
        transaction.execute("INSERT OR IGNORE INTO plugin_secret_handles(plugin_id,account_uuid,secret_ref,namespace,purpose,created_at) VALUES (?1,?2,?3,?4,?5,?6)", params![plugin_id,account_uuid,secret_ref,namespace,purpose,unix_timestamp_millis()])?;
        transaction.execute("DELETE FROM plugin_secret_creations WHERE plugin_id=?1 AND account_uuid=?2 AND secret_ref=?3", params![plugin_id,account_uuid,secret_ref])?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn abandon_plugin_secret_creation(
        &self,
        plugin_id: &str,
        account_uuid: &str,
        secret_ref: &str,
        needs_retirement: bool,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if needs_retirement {
            transaction.execute("INSERT OR IGNORE INTO plugin_credential_retirements(retirement_uuid,plugin_id,account_uuid,secret_ref,namespace,purpose,attempts,next_retry_at,created_at,updated_at) SELECT lower(hex(randomblob(16))),plugin_id,account_uuid,secret_ref,namespace,purpose,0,?1,?1,?1 FROM plugin_secret_creations WHERE plugin_id=?2 AND account_uuid=?3 AND secret_ref=?4", params![now,plugin_id,account_uuid,secret_ref])?;
        }
        transaction.execute("DELETE FROM plugin_secret_creations WHERE plugin_id=?1 AND account_uuid=?2 AND secret_ref=?3", params![plugin_id,account_uuid,secret_ref])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn forget_plugin_secret_handle(
        &self,
        plugin_id: &str,
        account_uuid: &str,
        secret_ref: &str,
    ) -> anyhow::Result<()> {
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute("DELETE FROM plugin_secret_handles WHERE plugin_id=?1 AND account_uuid=?2 AND secret_ref=?3", params![plugin_id, account_uuid, secret_ref])?;
        Ok(())
    }

    pub fn fail_credential_retirement(
        &self,
        retirement_uuid: &str,
        attempts: u32,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let exponent = attempts.min(8);
        let retry_ms = (1_i64 << exponent) * 1_000;
        let retry_at = now + retry_ms.min(5 * 60 * 1_000);
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE plugin_credential_retirements SET attempts=attempts+1,next_retry_at=?1,last_error_code='credential-retirement-pending',updated_at=?2 WHERE retirement_uuid=?3",
            params![retry_at,now,retirement_uuid],
        )?;
        transaction.execute(
            "UPDATE plugin_runtime_state SET state='degraded',pid=NULL,session_id=NULL,last_error_code='credential-retirement-pending',last_error_detail='Old Provider credentials are awaiting secure deletion',updated_at=?1 WHERE plugin_id=(SELECT plugin_id FROM plugin_credential_retirements WHERE retirement_uuid=?2)",
            params![now,retirement_uuid],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn begin_plugin_auth_flow(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
    ) -> anyhow::Result<bool> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_credential_retirements WHERE plugin_id=?1)",
            [plugin_id],
            |row| row.get::<_, bool>(0),
        )? {
            anyhow::bail!("credential-retirement-pending");
        }
        let current: Option<(
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        )> = transaction
            .query_row(
                "SELECT auth_state,display_name,account_kind,secret_ref,stable_account_identity FROM plugin_accounts WHERE plugin_id=?1 AND EXISTS(SELECT 1 FROM plugin_runtime_state r WHERE r.plugin_id=plugin_accounts.plugin_id AND r.state='ready' AND r.session_id=?2)",
                params![plugin_id,supervisor_session],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
            )
            .optional()?;
        let Some((
            prior_auth_state,
            prior_display_name,
            prior_account_kind,
            prior_secret_ref,
            prior_stable_identity,
        )) = current
        else {
            return Ok(false);
        };
        transaction.execute(
            "DELETE FROM plugin_auth_flow_ownership WHERE plugin_id=?1",
            [plugin_id],
        )?;
        transaction.execute(
            "INSERT INTO plugin_auth_flow_ownership(plugin_id,flow_id,supervisor_session,prior_auth_state,prior_display_name,prior_account_kind,state,prior_secret_ref,prior_stable_account_identity,created_at) VALUES (?1,?2,?3,?4,?5,?6,'active',?7,?8,?9)",
            params![plugin_id,flow_id,supervisor_session,prior_auth_state,prior_display_name,prior_account_kind,prior_secret_ref,prior_stable_identity,now],
        )?;
        transaction.execute(
            "UPDATE plugin_accounts SET auth_state='authorizing',last_error=NULL,updated_at=?1 WHERE plugin_id=?2",
            params![now,plugin_id],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn plugin_auth_flow_is_current(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
    ) -> anyhow::Result<bool> {
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_auth_flow_ownership WHERE plugin_id=?1 AND flow_id=?2 AND supervisor_session=?3 AND state='active')",
            params![plugin_id,flow_id,supervisor_session],
            |row| row.get(0),
        ).map_err(Into::into)
    }

    pub fn plugin_auth_flow_ownership(
        &self,
        plugin_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT flow_id,supervisor_session FROM plugin_auth_flow_ownership WHERE plugin_id=?1",
                [plugin_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn plugin_auth_secret_write_is_allowed(&self, plugin_id: &str) -> anyhow::Result<bool> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM plugin_auth_flow_ownership WHERE plugin_id=?1 AND state='cancelling')",
                [plugin_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Claims cancellation before the worker is joined. Once this commits,
    /// SecretBroker rejects any late OAuth credential write for this flow.
    pub fn claim_plugin_auth_cancellation(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
    ) -> anyhow::Result<bool> {
        Ok(self
            .connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .execute(
                "UPDATE plugin_auth_flow_ownership SET state='cancelling' WHERE plugin_id=?1 AND flow_id=?2 AND supervisor_session=?3 AND state IN ('active','completing','cancelling')",
                params![plugin_id, flow_id, supervisor_session],
            )?
            == 1)
    }

    pub fn claim_plugin_auth_completion(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
    ) -> anyhow::Result<bool> {
        Ok(self
            .connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .execute(
                "UPDATE plugin_auth_flow_ownership SET state='completing' WHERE plugin_id=?1 AND flow_id=?2 AND supervisor_session=?3 AND state='active'",
                params![plugin_id, flow_id, supervisor_session],
            )?
            == 1)
    }

    pub fn release_plugin_auth_completion(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
    ) -> anyhow::Result<bool> {
        Ok(self
            .connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .execute(
                "UPDATE plugin_auth_flow_ownership SET state='active' WHERE plugin_id=?1 AND flow_id=?2 AND supervisor_session=?3 AND state='completing'",
                params![plugin_id, flow_id, supervisor_session],
            )?
            == 1)
    }

    pub fn apply_plugin_auth_result_for_flow(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
        auth_state: &str,
        display_name: Option<&str>,
        account_kind: Option<&str>,
        stable_identity: Option<&str>,
    ) -> anyhow::Result<Option<Vec<BindingFence>>> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let eligible: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_auth_flow_ownership f JOIN plugin_runtime_state r ON r.plugin_id=f.plugin_id WHERE f.plugin_id=?1 AND f.flow_id=?2 AND f.supervisor_session=?3 AND f.state='completing' AND r.state='ready' AND r.session_id=?3)",
            params![plugin_id, flow_id, supervisor_session],
            |row| row.get(0),
        )?;
        if !eligible {
            return Ok(None);
        }
        let current: Option<String> = transaction
            .query_row(
                "SELECT stable_account_identity FROM plugin_accounts WHERE plugin_id=?1",
                [plugin_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let fences = if auth_state == "authenticated"
            && current.as_deref() != stable_identity
            && current.is_some()
        {
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
            "UPDATE plugin_accounts SET auth_state=?1,display_name=COALESCE(?2,display_name),account_kind=COALESCE(?3,account_kind),stable_account_identity=CASE WHEN ?1='authenticated' THEN ?4 ELSE stable_account_identity END,last_error=NULL,updated_at=?5 WHERE plugin_id=?6",
            params![auth_state,display_name,account_kind,stable_identity,now,plugin_id],
        )?;
        if !fences.is_empty() {
            Self::bump_sync_status_revision(&transaction)?;
        }
        transaction.commit()?;
        Ok(Some(fences))
    }

    pub fn finish_plugin_auth_flow(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
    ) -> anyhow::Result<bool> {
        Ok(self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "DELETE FROM plugin_auth_flow_ownership WHERE plugin_id=?1 AND flow_id=?2 AND supervisor_session=?3 AND state <> 'cancelling'",
            params![plugin_id,flow_id,supervisor_session],
        )? == 1)
    }

    pub fn discard_plugin_auth_flow(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
    ) -> anyhow::Result<bool> {
        Ok(self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "DELETE FROM plugin_auth_flow_ownership WHERE plugin_id=?1 AND flow_id=?2 AND supervisor_session=?3",
            params![plugin_id,flow_id,supervisor_session],
        )? == 1)
    }

    pub fn cancel_plugin_auth_flow(
        &self,
        plugin_id: &str,
        flow_id: &str,
        supervisor_session: &str,
    ) -> anyhow::Result<bool> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let prior: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>)> = transaction.query_row(
            "SELECT prior_auth_state,prior_display_name,prior_account_kind,prior_secret_ref,prior_stable_account_identity FROM plugin_auth_flow_ownership WHERE plugin_id=?1 AND flow_id=?2 AND supervisor_session=?3",
            params![plugin_id,flow_id,supervisor_session],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).optional()?;
        let Some((auth_state, display_name, account_kind, prior_secret_ref, prior_stable_identity)) =
            prior
        else {
            return Ok(false);
        };
        let current: Option<(String, String)> = transaction.query_row(
            "SELECT account_uuid,secret_ref FROM plugin_accounts WHERE plugin_id=?1 AND secret_ref IS NOT NULL",
            [plugin_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        if let Some((account_uuid, secret_ref)) = current
            .filter(|(_, secret_ref)| Some(secret_ref.as_str()) != prior_secret_ref.as_deref())
            .filter(|_| plugin_id == ONEDRIVE_PLUGIN_ID)
        {
            transaction.execute(
                "INSERT OR IGNORE INTO plugin_credential_retirements(retirement_uuid,plugin_id,account_uuid,secret_ref,namespace,purpose,attempts,next_retry_at,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,0,?7,?7,?7)",
                params![uuid::Uuid::now_v7().to_string(),plugin_id,account_uuid,secret_ref,ONEDRIVE_SECRET_NAMESPACE,ONEDRIVE_CREDENTIAL_PURPOSE,now],
            )?;
        }
        transaction.execute(
            "UPDATE plugin_accounts SET auth_state=?1,display_name=?2,account_kind=?3,secret_ref=?4,stable_account_identity=?5,last_error=NULL,updated_at=?6 WHERE plugin_id=?7",
            params![auth_state,display_name,account_kind,prior_secret_ref,prior_stable_identity,now,plugin_id],
        )?;
        transaction.execute(
            "DELETE FROM plugin_auth_flow_ownership WHERE plugin_id=?1",
            [plugin_id],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn recover_plugin_auth_flows_for_startup(&self) -> anyhow::Result<usize> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO plugin_credential_retirements(retirement_uuid,plugin_id,account_uuid,secret_ref,namespace,purpose,attempts,next_retry_at,created_at,updated_at) SELECT lower(hex(randomblob(16))),a.plugin_id,a.account_uuid,a.secret_ref,?1,?2,0,?3,?3,?3 FROM plugin_accounts a JOIN plugin_auth_flow_ownership f ON f.plugin_id=a.plugin_id WHERE a.plugin_id=?4 AND a.secret_ref IS NOT NULL AND a.secret_ref IS NOT f.prior_secret_ref",
            params![ONEDRIVE_SECRET_NAMESPACE, ONEDRIVE_CREDENTIAL_PURPOSE, now, ONEDRIVE_PLUGIN_ID],
        )?;
        let changed = transaction.execute(
            "UPDATE plugin_accounts SET auth_state=COALESCE((SELECT prior_auth_state FROM plugin_auth_flow_ownership f WHERE f.plugin_id=plugin_accounts.plugin_id),auth_state),display_name=(SELECT prior_display_name FROM plugin_auth_flow_ownership f WHERE f.plugin_id=plugin_accounts.plugin_id),account_kind=(SELECT prior_account_kind FROM plugin_auth_flow_ownership f WHERE f.plugin_id=plugin_accounts.plugin_id),secret_ref=(SELECT prior_secret_ref FROM plugin_auth_flow_ownership f WHERE f.plugin_id=plugin_accounts.plugin_id),stable_account_identity=(SELECT prior_stable_account_identity FROM plugin_auth_flow_ownership f WHERE f.plugin_id=plugin_accounts.plugin_id),updated_at=?1 WHERE plugin_id IN (SELECT plugin_id FROM plugin_auth_flow_ownership)",
            [now],
        )?;
        transaction.execute("DELETE FROM plugin_auth_flow_ownership", [])?;
        transaction.commit()?;
        Ok(changed)
    }

    pub fn reset_plugin_runtime_states_for_startup(
        &self,
        secret_store_available: bool,
    ) -> anyhow::Result<usize> {
        let now = unix_timestamp_millis();
        let onedrive_configuration = self.onedrive_configuration()?;
        let onedrive_configured = onedrive_configuration.configured;
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
                    Some("Microsoft public client ID is not configured"),
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
            "SELECT i.plugin_id,i.selected_version,v.manifest_json,i.install_root,v.entry_sha256,a.account_uuid,a.generation,i.requested_permissions_json,i.granted_permissions_json FROM plugin_installations i JOIN plugin_versions v ON v.plugin_id=i.plugin_id AND v.version=i.selected_version JOIN plugin_accounts a ON a.plugin_id=i.plugin_id JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id WHERE i.enabled_intent=1 AND r.state <> 'configuration-required' AND NOT EXISTS(SELECT 1 FROM plugin_credential_retirements q WHERE q.plugin_id=i.plugin_id) ORDER BY i.plugin_id",
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
            let install_root_changed = Path::new(install_root) != plugin.install_root;
            if install_root_changed
                && (plugin.trust_policy_id != "embedded-v1" || selected_version != plugin.version)
            {
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
                "UPDATE plugin_installations SET manifest_version=2,install_root=?1,trust_policy_id=?2,requested_permissions_json=?3,updated_at=?4 WHERE plugin_id=?5",
                params![plugin.install_root.to_string_lossy(),plugin.trust_policy_id,plugin.requested_permissions_json,now,plugin.plugin_id],
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

    #[cfg(test)]
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
            "SELECT i.plugin_id,i.selected_version,i.known_good_version,i.enabled_intent,i.requested_permissions_json,i.granted_permissions_json,r.state,a.auth_state,a.account_uuid,a.display_name,r.backoff_until,r.last_healthy_at,r.last_error_code,r.last_error_detail,r.capabilities_json,a.account_kind,v.manifest_json FROM plugin_installations i JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id LEFT JOIN plugin_accounts a ON a.plugin_id=i.plugin_id LEFT JOIN plugin_versions v ON v.plugin_id=i.plugin_id AND v.version=i.selected_version ORDER BY i.plugin_id",
        )?;
        let rows = statement.query_map([], |row| {
            let plugin_id: String = row.get(0)?;
            let requested: String = row.get(4)?;
            let granted: String = row.get(5)?;
            let manifest_json: Option<String> = row.get(16)?;
            let name = selected_plugin_name(&plugin_id, manifest_json.as_deref());
            Ok(HostPluginProjection {
                plugin_id,
                name,
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
                account_kind: row.get(15)?,
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

    #[cfg(test)]
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
        account_kind: Option<&str>,
    ) -> anyhow::Result<bool> {
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE plugin_accounts SET auth_state=?1,display_name=COALESCE(?2,display_name),account_kind=COALESCE(?3,account_kind),last_error=NULL,updated_at=?4 WHERE plugin_id=?5 AND EXISTS(SELECT 1 FROM plugin_runtime_state r WHERE r.plugin_id=plugin_accounts.plugin_id AND r.state='ready' AND r.session_id=?6)",
            params![auth_state,display_name,account_kind,unix_timestamp_millis(),plugin_id,expected_session],
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
    #[cfg(test)]
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
            cache_remote_descriptor(&transaction, account_uuid, account_generation, discovered)?;
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
            "INSERT INTO provider_object_receipts(library_uuid,binding_epoch,object_key,etag,sha256,state,generation,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8) ON CONFLICT(library_uuid,binding_epoch,object_key) DO UPDATE SET etag=excluded.etag,sha256=excluded.sha256,state=CASE WHEN excluded.state='durable' AND provider_object_receipts.sha256=excluded.sha256 AND provider_object_receipts.state IN ('created','already-exists-matching') THEN provider_object_receipts.state ELSE excluded.state END,generation=excluded.generation,updated_at=excluded.updated_at",
            params![library_uuid,binding_epoch.as_bytes().as_slice(),object_key,etag,sha256.as_slice(),state,transport_generation,now],
        )?;
        Ok(())
    }

    pub fn has_published_receipt(
        &self,
        library_uuid: &str,
        binding: &BindingTransport,
        key: &str,
        hash: &[u8; 32],
    ) -> anyhow::Result<bool> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM provider_object_receipts WHERE library_uuid=?1 AND binding_epoch=?2 AND generation<=?3 AND object_key=?4 AND sha256=?5 AND state IN ('created','already-exists-matching'))",params![library_uuid,binding.binding_epoch.as_bytes().as_slice(),binding.transport_generation,key,hash.as_slice()],|row|row.get(0))?)
    }

    pub fn cache_bound_descriptor(
        &self,
        library_uuid: &str,
        binding: &BindingTransport,
        discovered: &DiscoveredRemoteLibrary,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let generation: Option<i64> = transaction.query_row("SELECT a.generation FROM library_sync_bindings b JOIN plugin_accounts a ON a.account_uuid=b.account_uuid WHERE b.library_uuid=?1 AND b.binding_epoch=?2 AND b.transport_generation=?3 AND a.account_uuid=?4",params![library_uuid,binding.binding_epoch.as_bytes().as_slice(),binding.transport_generation,binding.account_uuid],|row|row.get(0)).optional()?;
        let generation = generation.context("stale-provider-transport-generation")?;
        cache_remote_descriptor(&transaction, &binding.account_uuid, generation, discovered)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn chosen_descriptor(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Option<(String, Option<String>, Option<String>)>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let restoring: Option<(String,Option<String>,Option<String>)> = connection.query_row("SELECT descriptor_key,checkpoint_key,checkpoint_sha256 FROM sync_restore_plans WHERE library_uuid=?1 AND catalog_ready=0 AND state NOT IN ('failed','cancelled') ORDER BY created_at DESC LIMIT 1",[library_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        if restoring.is_some() {
            return Ok(restoring);
        }
        Ok(connection.query_row("SELECT r.descriptor_key,r.checkpoint_key,r.checkpoint_sha256 FROM provider_remote_libraries r JOIN library_sync_bindings b ON b.account_uuid=r.account_uuid AND b.library_uuid=r.library_uuid JOIN plugin_accounts a ON a.account_uuid=r.account_uuid AND a.generation=r.account_generation WHERE r.library_uuid=?1 AND r.compatible=1",[library_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?)
    }

    pub fn verified_remote_descriptor(
        &self,
        account_uuid: &str,
        library_uuid: &str,
        metadata: &crate::sync::provider::ObjectMetadata,
    ) -> anyhow::Result<Option<DiscoveredRemoteLibrary>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let row:Option<String> = connection.query_row("SELECT v.candidate_json FROM provider_verified_descriptors v JOIN plugin_accounts a ON a.account_uuid=v.account_uuid AND a.generation=v.account_generation WHERE v.account_uuid=?1 AND v.library_uuid=?2 AND v.descriptor_key=?3",params![account_uuid,library_uuid,metadata.key],|row|row.get(0)).optional()?;
        let candidate = row
            .map(|json| serde_json::from_str::<DiscoveredRemoteLibrary>(&json))
            .transpose()?;
        Ok(candidate.filter(|candidate| {
            metadata
                .etag
                .as_deref()
                .is_some_and(|etag| !etag.is_empty() && !etag.starts_with("W/"))
                && candidate.metadata.size == metadata.size
                && candidate.metadata.sha256 == metadata.sha256
                && candidate.metadata.etag == metadata.etag
        }))
    }

    pub fn chosen_descriptor_identity(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Option<(String, Vec<u8>)>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let pinned = connection.query_row("SELECT p.descriptor_key,p.descriptor_sha256 FROM sync_restore_plans p JOIN library_sync_bindings b ON b.library_uuid=p.library_uuid AND b.account_uuid=p.account_uuid WHERE p.library_uuid=?1 AND p.catalog_ready=0 AND p.state NOT IN ('failed','cancelled') ORDER BY p.created_at DESC LIMIT 1",[library_uuid],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        if pinned.is_some() {
            return Ok(pinned);
        }
        Ok(connection.query_row("SELECT r.descriptor_key,r.descriptor_sha256 FROM provider_remote_libraries r JOIN library_sync_bindings b ON b.account_uuid=r.account_uuid AND b.library_uuid=r.library_uuid JOIN plugin_accounts a ON a.account_uuid=r.account_uuid AND a.generation=r.account_generation WHERE r.library_uuid=?1 AND r.compatible=1",[library_uuid],|row|Ok((row.get(0)?,row.get(1)?))).optional()?)
    }

    pub fn complete_restore_descriptor_pin(
        &self,
        library_uuid: &str,
        binding: &BindingTransport,
        candidate: &DiscoveredRemoteLibrary,
    ) -> anyhow::Result<()> {
        let (_, key) = crate::sync::descriptor::split_root_key(&candidate.metadata.key)?;
        let evidence = candidate
            .evidence
            .as_ref()
            .context("descriptor evidence missing")?;
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection.execute("UPDATE sync_restore_plans SET checkpoint_key=?1,checkpoint_sha256=?2 WHERE library_uuid=?3 AND account_uuid=?4 AND catalog_ready=0 AND state NOT IN ('failed','cancelled') AND descriptor_key=?5 AND descriptor_sha256=?6 AND (checkpoint_key IS NULL OR (checkpoint_key=?1 AND checkpoint_sha256=?2)) AND EXISTS(SELECT 1 FROM library_sync_bindings b WHERE b.library_uuid=?3 AND b.account_uuid=?4 AND b.binding_epoch=?7 AND b.transport_generation=?8)",params![evidence.checkpoint_key,evidence.checkpoint_sha256,library_uuid,binding.account_uuid,key,candidate.metadata.sha256.as_slice(),binding.binding_epoch.as_bytes().as_slice(),binding.transport_generation])?;
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
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 7 {
            connection.execute_batch(HOST_MIGRATION_0007)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 8 {
            connection.execute_batch(HOST_MIGRATION_0008)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 9 {
            connection.execute_batch(HOST_MIGRATION_0009)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 10 {
            connection.execute_batch(HOST_MIGRATION_0010)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 11 {
            connection.execute_batch(HOST_MIGRATION_0011)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 12 {
            connection.execute_batch(HOST_MIGRATION_0012)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 13 {
            connection.execute_batch(HOST_MIGRATION_0013)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 14 {
            connection.execute_batch(HOST_MIGRATION_0014)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 15 {
            connection.execute_batch(HOST_MIGRATION_0015)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 16 {
            connection.execute_batch(HOST_MIGRATION_0016)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 17 {
            connection.execute_batch(HOST_MIGRATION_0017)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 18 {
            connection.execute_batch(HOST_MIGRATION_0018)?;
        }
        let now = unix_timestamp_millis();
        connection.execute("INSERT OR IGNORE INTO plugin_credential_retirements(retirement_uuid,plugin_id,account_uuid,secret_ref,namespace,purpose,attempts,next_retry_at,created_at,updated_at) SELECT lower(hex(randomblob(16))),plugin_id,account_uuid,secret_ref,namespace,purpose,0,?1,?1,?1 FROM plugin_secret_creations", [now])?;
        connection.execute("DELETE FROM plugin_secret_creations", [])?;
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
            "UPDATE library_sync_bindings SET capture_generation=?1,poll_after=0,updated_at=?2 WHERE library_uuid=?3 AND binding_epoch=?4 AND state='preparing'",
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
            "SELECT capture_generation FROM library_sync_bindings WHERE library_uuid=?1 AND state IN ('preparing','capturing','seed_ready','publishing') AND capture_generation IS NOT NULL",
            [library_uuid],|row|row.get(0),
        ).optional()?)
    }

    pub fn sync_capture_boundary_pending(&self, library_uuid: &str) -> anyhow::Result<bool> {
        Ok(self.connection.lock().unwrap_or_else(|p| p.into_inner()).query_row(
            "SELECT EXISTS(SELECT 1 FROM library_sync_bindings WHERE library_uuid=?1 AND state IN ('preparing','capturing') AND capture_generation IS NULL)",
            [library_uuid], |row| row.get(0),
        )?)
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
            "UPDATE library_sync_bindings SET state='active',updated_at=?1 WHERE library_uuid=?2 AND binding_epoch=?3 AND transport_generation=?4 AND state IN ('seed_ready','publishing') AND EXISTS (SELECT 1 FROM provider_object_receipts r WHERE r.library_uuid=library_sync_bindings.library_uuid AND r.binding_epoch=library_sync_bindings.binding_epoch AND r.object_key='library-descriptor' AND r.generation<=?4 AND r.state IN ('created','already-exists-matching')) AND EXISTS (SELECT 1 FROM provider_object_receipts r WHERE r.library_uuid=library_sync_bindings.library_uuid AND r.binding_epoch=library_sync_bindings.binding_epoch AND r.object_key=?5 AND r.generation<=?4 AND r.state IN ('created','already-exists-matching'))",
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
                "SELECT i.plugin_id,a.account_uuid,a.display_name,a.account_kind,r.state,a.auth_state,r.last_healthy_at,r.last_error_code FROM plugin_installations i JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id JOIN plugin_accounts a ON a.plugin_id=i.plugin_id WHERE EXISTS (SELECT 1 FROM json_each(r.capabilities_json) WHERE value='sync-provider.v1') ORDER BY i.plugin_id",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok(SyncProviderStatus {
                        plugin_id: row.get(0)?,
                        account_uuid: row.get(1)?,
                        account_display_name: row.get(2)?,
                        account_kind: row.get(3)?,
                        runtime_state: row.get(4)?,
                        auth_state: row.get(5)?,
                        last_healthy_at: row.get(6)?,
                        last_error_code: row.get(7)?,
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

    pub fn remote_library_root_inventory(
        &self,
        account_uuid: &str,
        library_uuid: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<RemoteLibraryRootInventory> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        // Keep the page and its fence under one Host snapshot. A provider refresh
        // cannot otherwise replace the root set between the page read and hash.
        let placement_base: Option<String> = connection
            .query_row(
                "SELECT settings.default_placement_base FROM library_sync_bindings binding JOIN library_local_settings settings ON settings.library_uuid=binding.library_uuid WHERE binding.account_uuid=?1 AND COALESCE(binding.remote_namespace,binding.library_uuid)=?2 LIMIT 1",
                params![account_uuid, library_uuid],
                |row| row.get(0),
            )
            .optional()?;
        let (generation, descriptor): (i64, Vec<u8>) = connection
            .query_row(
                "SELECT library.account_generation,library.descriptor_sha256 FROM provider_remote_libraries library JOIN plugin_accounts account ON account.account_uuid=library.account_uuid WHERE library.account_uuid=?1 AND library.library_uuid=?2 AND library.account_generation=account.generation",
                params![account_uuid, library_uuid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context("remote-library-not-found")?;
        let mut hasher = Sha256::new();
        hasher.update(b"megle-remote-root-inventory-v1\0");
        hasher.update(account_uuid.as_bytes());
        hasher.update(b"\0");
        hasher.update(library_uuid.as_bytes());
        hasher.update(b"\0");
        hasher.update(generation.to_le_bytes());
        hasher.update(&descriptor);
        let mut statement = connection.prepare(
            "SELECT root_uuid,root_folder_uuid,requested_name,materialized_name,materialization_fingerprint,logical_revision,COALESCE(logical_bytes,-1),compatible,COALESCE(error_code,'') FROM provider_remote_sync_roots WHERE account_uuid=?1 AND library_uuid=?2 ORDER BY root_uuid",
        )?;
        let mut rows = statement.query(params![account_uuid, library_uuid])?;
        while let Some(row) = rows.next()? {
            for value in 0..5 {
                let text: String = row.get(value)?;
                hasher.update(text.as_bytes());
                hasher.update(b"\0");
            }
            let logical_revision: i64 = row.get(5)?;
            let logical_bytes: i64 = row.get(6)?;
            let compatible: i64 = row.get(7)?;
            let error_code: String = row.get(8)?;
            hasher.update(logical_revision.to_le_bytes());
            hasher.update(logical_bytes.to_le_bytes());
            hasher.update(compatible.to_le_bytes());
            hasher.update(error_code.as_bytes());
            hasher.update(b"\0");
        }
        let mut statement = connection.prepare(
            "SELECT root.account_uuid,root.library_uuid,root.root_uuid,root.root_folder_uuid,root.requested_name,root.materialized_name,root.materialization_fingerprint,root.logical_revision,root.logical_bytes,root.compatible,root.error_code FROM provider_remote_sync_roots root JOIN provider_remote_libraries library ON library.account_uuid=root.account_uuid AND library.library_uuid=root.library_uuid JOIN plugin_accounts account ON account.account_uuid=root.account_uuid WHERE root.account_uuid=?1 AND root.library_uuid=?2 AND library.account_generation=account.generation AND (?3 IS NULL OR root.root_uuid>?3) ORDER BY root.root_uuid LIMIT ?4",
        )?;
        let items = statement
            .query_map(
                params![account_uuid, library_uuid, cursor, i64::try_from(limit)?],
                |row| {
                    let materialized_name: String = row.get(5)?;
                    Ok(RemoteLibraryRootSummary {
                        account_uuid: row.get(0)?,
                        library_uuid: row.get(1)?,
                        root_uuid: row.get(2)?,
                        root_folder_uuid: row.get(3)?,
                        requested_name: row.get(4)?,
                        proposed_local_path: placement_base.as_ref().map(|base| {
                            PathBuf::from(base)
                                .join(&materialized_name)
                                .to_string_lossy()
                                .into_owned()
                        }),
                        materialized_name,
                        materialization_fingerprint: row.get(6)?,
                        logical_revision: row.get::<_, i64>(7)?.max(0) as u64,
                        logical_bytes: row
                            .get::<_, Option<i64>>(8)?
                            .map(|value| value.max(0) as u64),
                        selected_by_default: row.get::<_, i64>(9)? != 0,
                        compatible: row.get::<_, i64>(9)? != 0,
                        error_code: row.get(10)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let inventory = RemoteLibraryRootInventory {
            items,
            inventory_fence: format!("{:x}", hasher.finalize()),
        };
        Ok(inventory)
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

    /// An explicit retry restores eligibility from durable publication facts.
    /// Ordinary wake signals must never reopen an errored or fenced binding.
    /// Returns whether the interrupted initial Catalog capture still needs setup.
    pub fn retry_binding(
        &self,
        library_uuid: &str,
        expected_revision: u64,
        expected: &BindingTransport,
    ) -> anyhow::Result<bool> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let revision: u64 = transaction.query_row(
            "SELECT context_revision FROM host_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if revision != expected_revision {
            anyhow::bail!("stale-library-context");
        }
        let (state, checkpoint, capture_generation): (String, Option<Vec<u8>>, Option<i64>) =
            transaction.query_row(
                "SELECT state,latest_checkpoint_uuid,capture_generation FROM library_sync_bindings WHERE library_uuid=?1 AND binding_epoch=?2 AND transport_generation=?3 AND plugin_id=?4 AND account_uuid=?5 AND actor_epoch=?6",
                params![library_uuid,expected.binding_epoch.as_bytes().as_slice(),expected.transport_generation,expected.plugin_id,expected.account_uuid,expected.actor_epoch.as_bytes().as_slice()],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            ).optional()?.ok_or_else(|| anyhow::anyhow!("stale-provider-transport-generation"))?;
        let unbinding: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_unbind_plans WHERE library_uuid=?1 AND state IN ('cutover','reconciling'))",
            [library_uuid],
            |row| row.get(0),
        )?;
        if unbinding
            || !matches!(state.as_str(), "error" | "preparing" | "capturing")
            || !sync_public_state(Some(&state)).3.contains(&"retry")
        {
            anyhow::bail!("sync-action-not-allowed");
        }
        let provider_ready: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_installations i JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id JOIN plugin_accounts a ON a.plugin_id=i.plugin_id WHERE i.plugin_id=?1 AND a.account_uuid=?2 AND i.enabled_intent=1 AND r.state='ready' AND a.auth_state='authenticated' AND EXISTS(SELECT 1 FROM json_each(r.capabilities_json) WHERE value='sync-provider.v1'))",
            params![expected.plugin_id,expected.account_uuid],
            |row| row.get(0),
        )?;
        if !provider_ready {
            anyhow::bail!("provider-not-ready");
        }
        let next_state = if state != "error" {
            state.as_str()
        } else if let Some(checkpoint) = checkpoint.as_ref() {
            let checkpoint_key = format!(
                "checkpoints/{}.checkpoint",
                uuid::Uuid::from_slice(checkpoint)?
            );
            // Pausing advances the transport generation, not the immutable
            // remote publication. Earlier receipts remain evidence within the
            // same binding epoch; a future or foreign epoch is never accepted.
            let published: bool = transaction.query_row(
                "SELECT COUNT(*)=2 FROM provider_object_receipts WHERE library_uuid=?1 AND binding_epoch=?2 AND generation<=?3 AND object_key IN ('library-descriptor',?4) AND state IN ('created','already-exists-matching')",
                params![library_uuid,expected.binding_epoch.as_bytes().as_slice(),expected.transport_generation,checkpoint_key],
                |row| row.get(0),
            )?;
            if published {
                "active"
            } else {
                "seed_ready"
            }
        } else if capture_generation.is_none() && expected.remote_namespace.is_some() {
            // A restored Catalog has no locally published seed receipt. Keep
            // restore work in its own phase instead of starting a new capture.
            let restore_state: Option<String> = transaction.query_row(
                "SELECT state FROM sync_restore_plans WHERE library_uuid=?1 AND account_uuid=?2 ORDER BY created_at DESC LIMIT 1",
                params![library_uuid,expected.account_uuid],
                |row| row.get(0),
            ).optional()?;
            match restore_state.as_deref() {
                Some("complete") => "active",
                Some("restoring" | "catalog-ready") => "restoring",
                Some(_) => anyhow::bail!("sync-action-not-allowed"),
                None => "preparing",
            }
        } else {
            "preparing"
        };
        let needs_capture = next_state == "preparing" && capture_generation.is_none();
        transaction.execute(
            "UPDATE library_sync_bindings SET state=?2,poll_after=0,retry_after=NULL,last_error_code=NULL,updated_at=?3 WHERE library_uuid=?1",
            params![library_uuid,next_state,unix_timestamp_millis()],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(needs_capture)
    }

    pub fn wake_binding(&self, library_uuid: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if transaction.execute("UPDATE library_sync_bindings SET poll_after=0,retry_after=NULL,last_error_code=NULL,updated_at=?2 WHERE library_uuid=?1 AND state IN ('preparing','capturing','seed_ready','publishing','active','restoring')",params![library_uuid,unix_timestamp_millis()])? == 0 { anyhow::bail!("sync-action-not-allowed"); }
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn wake_all_sync_bindings(&self) -> anyhow::Result<usize> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE library_sync_bindings SET poll_after=0,retry_after=NULL,last_error_code=NULL,updated_at=?1 WHERE state IN ('preparing','capturing','seed_ready','publishing','active','restoring')",
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
        let changed = transaction.execute(
            "UPDATE library_sync_bindings SET last_success_at=?2,last_error_code=NULL,retry_after=NULL,poll_after=?3,updated_at=?2 WHERE library_uuid=?1 AND state IN ('preparing','capturing','seed_ready','publishing','active','restoring')",
            params![library_uuid,now,next_poll],
        )?;
        if changed > 0 {
            Self::bump_sync_status_revision(&transaction)?;
        }
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
        let changed = transaction.execute(
            "UPDATE library_sync_bindings SET state='error',last_error_code=?2,retry_after=?3,poll_after=NULL,updated_at=?4 WHERE library_uuid=?1 AND state IN ('preparing','capturing','seed_ready','publishing','active','restoring')",
            params![library_uuid,error_code,now+15_000,now],
        )?;
        if changed > 0 {
            Self::bump_sync_status_revision(&transaction)?;
        }
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

    #[cfg(test)]
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
        transaction.execute("INSERT INTO sync_restore_plans(restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,descriptor_sha256,state,phase,restore_mode,inventory_fence,request_hash,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'restoring','operation-and-plans-recorded',?8,?9,?10,?11,?11)",params![restore_uuid,request.operation_uuid,request.account_uuid,request.library_uuid,remote_name,request.managed_root.to_string_lossy(),hash,request.restore_mode,request.inventory_fence,request.request_hash,now])?;
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
        let (device,stream,host_actor):(Vec<u8>,Vec<u8>,Option<Vec<u8>>)=transaction.query_row("SELECT device_uuid,stream_uuid,active_actor_epoch FROM host_sync_local_state WHERE singleton=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        if host_actor.is_none() {
            let host_actor = uuid::Uuid::now_v7();
            transaction.execute("INSERT INTO host_sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,state,next_actor_seq,published_through_seq,created_at) VALUES (?1,?2,?3,'active',1,0,?4)",params![host_actor.as_bytes().as_slice(),device,stream,unix_timestamp_millis()])?;
            transaction.execute("UPDATE host_sync_local_state SET active_actor_epoch=?1,capture_enabled=1,updated_at=?2 WHERE singleton=1",params![host_actor.as_bytes().as_slice(),unix_timestamp_millis()])?;
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
        root_results: &[SyncRestoreRootResult],
        volume_results: &[SyncRestoreVolumeResult],
    ) -> anyhow::Result<SyncRestorePreflight> {
        let now = unix_timestamp_millis();
        let expires_at = now + 10 * 60 * 1000;
        let token = uuid::Uuid::now_v7().to_string();
        let request_hash = crate::sync_folders::request_hash(request)?;
        let result = SyncRestorePreflight {
            preflight_token: Some(token),
            expires_at,
            total_items,
            total_bytes,
            ok: true,
            root_results: root_results.to_vec(),
            volume_results: volume_results.to_vec(),
        };
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "INSERT INTO sync_restore_preflights(token,request_json,request_hash,result_json,total_items,total_bytes,expires_at,state,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'ready',?8)",
            params![result.preflight_token,serde_json::to_string(request)?,request_hash,serde_json::to_string(&result)?,total_items as i64,total_bytes as i64,expires_at,now],
        )?;
        Ok(result)
    }

    #[cfg(test)]
    pub fn claim_restore_preflight(
        &self,
        token: &str,
        operation_uuid: &str,
        request: &serde_json::Value,
    ) -> anyhow::Result<bool> {
        let now = unix_timestamp_millis();
        let request_json = serde_json::to_string(request)?;
        let request_hash = crate::sync_folders::request_hash(request)?;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if let Some((existing_json, kind)) = transaction.query_row(
            "SELECT request_json,kind FROM sync_orchestration_operations WHERE operation_uuid=?1",
            [operation_uuid],
            |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)),
        ).optional()? {
            if kind != "restore" || existing_json != request_json { bail!("idempotency-key-reused"); }
            let claimed: Option<(String,String)> = transaction.query_row("SELECT COALESCE(operation_uuid,''),COALESCE(request_hash,'') FROM sync_restore_preflights WHERE token=?1",[token],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            if claimed.as_ref().is_none_or(|value| value.0 != operation_uuid || value.1 != request_hash) { bail!("restore-preflight-expired-or-changed"); }
            transaction.commit()?;
            return Ok(false);
        }
        let changed = transaction.execute(
            "UPDATE sync_restore_preflights SET state='claimed',operation_uuid=?1,request_hash=?2,consumed_at=?3 WHERE token=?4 AND request_json=?5 AND request_hash=?2 AND state='ready' AND expires_at>=?3",
            params![operation_uuid,request_hash,now,token,request_json],
        )?;
        if changed != 1 {
            bail!("restore-preflight-expired-or-changed");
        }
        transaction.execute(
            "INSERT INTO sync_orchestration_operations(operation_uuid,library_uuid,kind,state,phase,request_json,created_at,updated_at) VALUES (?1,NULL,'restore','running','operation-and-plans-recorded',?2,?3,?3)",
            params![operation_uuid,request_json,now],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn complete_restore_preflight(
        &self,
        token: &str,
        operation_uuid: &str,
    ) -> anyhow::Result<()> {
        let changed = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .execute(
                "UPDATE sync_restore_preflights SET state='completed' WHERE token=?1 AND operation_uuid=?2 AND state IN ('claimed','completed')",
                params![token, operation_uuid],
            )?;
        if changed != 1 {
            bail!("restore-preflight-expired-or-changed");
        }
        Ok(())
    }

    pub fn fail_restore_preflight(&self, token: &str, operation_uuid: &str) -> anyhow::Result<()> {
        let changed = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .execute(
                "UPDATE sync_restore_preflights SET state='failed-recoverable' WHERE token=?1 AND operation_uuid=?2 AND state='claimed'",
                params![token, operation_uuid],
            )?;
        if changed > 1 {
            bail!("restore-preflight-state-invalid");
        }
        Ok(())
    }

    pub fn fail_restore_operation_before_catalog(
        &self,
        operation_uuid: &str,
        error_code: &str,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE sync_restore_root_plans SET phase='compensated',updated_at=?1 WHERE restore_uuid IN (SELECT restore_uuid FROM sync_restore_plans WHERE operation_uuid=?2 AND catalog_ready=0)",
            params![now, operation_uuid],
        )?;
        transaction.execute(
            "UPDATE sync_restore_plans SET state='failed',phase='compensated',error_code=?1,updated_at=?2 WHERE operation_uuid=?3 AND catalog_ready=0 AND state NOT IN ('complete','cancelled')",
            params![error_code, now, operation_uuid],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
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
        transaction.execute("UPDATE sync_restore_plans SET state='catalog-ready',phase='catalog-ready',catalog_ready=1,updated_at=?2 WHERE library_uuid=?1 AND state IN ('restoring','failed')",params![library_uuid,now])?;
        transaction.execute("UPDATE sync_restore_root_plans SET phase='catalog-ready',updated_at=?2 WHERE restore_uuid IN (SELECT restore_uuid FROM sync_restore_plans WHERE library_uuid=?1) AND phase NOT IN ('completed','compensated')", params![library_uuid, now])?;
        transaction.execute(
            "UPDATE libraries SET availability='available',updated_at=?2 WHERE library_uuid=?1",
            params![library_uuid, now],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn advance_restore_phase(&self, library_uuid: &str, phase: &str) -> anyhow::Result<()> {
        const PHASES: &[&str] = &[
            "operation-and-plans-recorded",
            "library-shell-created",
            "inventory-imported",
            "markers-staged",
            "host-bindings-staged",
            "catalog-staged",
            "catalog-ready",
            "reconciling",
            "workers-published",
            "completed",
        ];
        if !PHASES.contains(&phase) {
            bail!("invalid-restore-phase");
        }
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE sync_restore_plans SET phase=?2,updated_at=?3 WHERE library_uuid=?1 AND state NOT IN ('cancelled')",
            params![library_uuid, phase, now],
        )?;
        if changed == 0 {
            bail!("restore-not-found");
        }
        transaction.execute(
            "UPDATE sync_orchestration_operations SET phase=?2,updated_at=?3 WHERE operation_uuid IN (SELECT operation_uuid FROM sync_restore_plans WHERE library_uuid=?1)",
            params![library_uuid, phase, now],
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

    pub fn restore_root_cleanup_entries(
        &self,
        restore_uuid: &str,
    ) -> anyhow::Result<Vec<RestoreRootCleanup>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT root_uuid,root_folder_uuid,target_path,local_binding_uuid,binding_generation FROM sync_restore_root_plans WHERE restore_uuid=?1 AND selected=1 ORDER BY root_uuid",
        )?;
        let entries = statement
            .query_map([restore_uuid], |row| {
                Ok(RestoreRootCleanup {
                    root_uuid: row.get(0)?,
                    root_folder_uuid: row.get(1)?,
                    target_path: PathBuf::from(row.get::<_, String>(2)?),
                    local_binding_uuid: row.get(3)?,
                    binding_generation: row.get::<_, i64>(4)?.max(1) as u64,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        Ok(entries)
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

    pub fn is_root_aware_restore(&self, library_uuid: &str) -> anyhow::Result<bool> {
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row(
            "SELECT restore_mode='root-aware' FROM sync_restore_plans WHERE library_uuid=?1 ORDER BY created_at DESC LIMIT 1",
            [library_uuid],
            |row| row.get(0),
        ).optional().map(|value| value.unwrap_or(false)).map_err(Into::into)
    }

    #[cfg(test)]
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
        // Any restore interrupted before Catalog promotion is compensatable.
        // Mark it failed so startup cleanup removes only its identity-bound
        // Library shell, staged root markers, and local bindings.
        transaction.execute("UPDATE sync_restore_plans SET state='failed',phase='startup-recovery',error_code='interrupted-before-catalog',updated_at=?1 WHERE state IN ('planned','restoring') AND catalog_ready=0", [now])?;
        transaction.execute("UPDATE sync_restore_preflights SET state='failed-recoverable' WHERE state='claimed' AND operation_uuid IN (SELECT operation_uuid FROM sync_restore_plans WHERE state='failed' AND phase='startup-recovery')", [])?;
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

fn selected_plugin_name(plugin_id: &str, manifest_json: Option<&str>) -> String {
    manifest_json
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|manifest| {
            manifest
                .get("name")?
                .as_str()
                .map(|name| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty() && name.len() <= 128)
        .unwrap_or_else(|| plugin_id.chars().take(128).collect())
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

    #[test]
    fn managed_root_v2_schema_is_durable_and_enabled() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        assert!(registry.sync_folder_v2_enabled());
        register_test_library(&registry, "00000000-0000-7000-8000-000000000001");
        let library_uuid = "00000000-0000-7000-8000-000000000001";
        let root_uuid = "00000000-0000-7000-8000-000000000002";
        let root_folder_uuid = "00000000-0000-7000-8000-000000000003";
        let binding_uuid = "00000000-0000-7000-8000-000000000004";
        let connection = registry.connection.lock().unwrap();
        let library_root_uuid = connection
            .query_row(
                "SELECT root_uuid FROM libraries WHERE library_uuid=?1",
                [library_uuid],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        connection.execute(
            "INSERT INTO library_local_settings(library_uuid,library_root_uuid,default_placement_base,canonical_placement_key,created_at,updated_at) VALUES (?1,?2,'D:/Library','d:/library',1,1)",
            params![library_uuid, library_root_uuid],
        ).unwrap();
        connection.execute(
            "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?1,?2,?3,'Photos','Photos','photos',?4,'active','unselected',1)",
            params![library_uuid,root_uuid,root_folder_uuid,"a".repeat(64)],
        ).unwrap();
        connection.execute(
            "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,applied_materialized_name,applied_materialized_name_key,applied_materialization_fingerprint,created_at,updated_at) VALUES (?1,?2,?3,'D:/Library/Photos','d:/library/photos','D:/Library','d:/library','active','Photos','photos',?4,'Photos','photos',?4,1,1)",
            params![library_uuid,root_uuid,binding_uuid,"a".repeat(64)],
        ).unwrap();
        drop(connection);

        let binding = registry
            .local_root_binding(library_uuid, root_uuid)
            .unwrap()
            .expect("read local root binding");
        assert_eq!(binding.local_binding_uuid, binding_uuid);
        assert_eq!(binding.binding_generation, 1);
        assert_eq!(
            registry
                .local_root_bindings(library_uuid)
                .unwrap()
                .into_iter()
                .map(|binding| binding.state)
                .collect::<Vec<_>>(),
            vec!["active"]
        );

        let request = serde_json::json!({"selected": true, "path": "D:/Library/Photos"});
        let operation_uuid = "00000000-0000-7000-8000-000000000020";
        let stale_revision = registry
            .begin_root_operation_once(
                "00000000-0000-7000-8000-000000000018",
                library_uuid,
                root_uuid,
                "select",
                "planned",
                &"8".repeat(64),
                &request,
                1,
                Some(1),
            )
            .unwrap_err();
        assert!(stale_revision
            .to_string()
            .contains("stale-operation-revision"));
        let missing_generation = registry
            .begin_root_operation_once(
                "00000000-0000-7000-8000-000000000019",
                library_uuid,
                root_uuid,
                "select",
                "planned",
                &"9".repeat(64),
                &request,
                0,
                None,
            )
            .unwrap_err();
        assert!(missing_generation
            .to_string()
            .contains("stale-binding-generation"));
        let (operation, inserted) = registry
            .begin_root_operation_once(
                operation_uuid,
                library_uuid,
                root_uuid,
                "select",
                "planned",
                &"c".repeat(64),
                &request,
                0,
                Some(1),
            )
            .unwrap();
        assert!(inserted);
        assert_eq!(operation.root_uuid, root_uuid);
        let (retried, inserted) = registry
            .begin_root_operation_once(
                operation_uuid,
                library_uuid,
                root_uuid,
                "select",
                "planned",
                &"c".repeat(64),
                &request,
                0,
                Some(1),
            )
            .unwrap();
        assert!(!inserted);
        assert_eq!(operation, retried);
        assert!(registry
            .begin_root_operation_once(
                operation_uuid,
                library_uuid,
                root_uuid,
                "select",
                "planned",
                &"d".repeat(64),
                &serde_json::json!({"selected": false}),
                0,
                Some(1),
            )
            .is_err());
        assert!(registry
            .begin_root_operation_once(
                "00000000-0000-7000-8000-000000000021",
                library_uuid,
                root_uuid,
                "select",
                "planned",
                &"e".repeat(64),
                &request,
                0,
                Some(1),
            )
            .is_err());

        {
            let connection = registry.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE root_operations SET state='completed',phase='completed' WHERE operation_uuid=?1",
                    [operation_uuid],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE local_root_bindings SET state='local-only-fork' WHERE library_uuid=?1 AND root_uuid=?2",
                    params![library_uuid, root_uuid],
                )
                .unwrap();
        }
        assert_eq!(
            registry
                .local_root_bindings(library_uuid)
                .unwrap()
                .into_iter()
                .map(|binding| binding.state)
                .collect::<Vec<_>>(),
            vec!["local-only-fork"]
        );
        let blocked_operation_uuid = "00000000-0000-7000-8000-000000000022";
        let blocked = registry
            .begin_root_operation_once(
                blocked_operation_uuid,
                library_uuid,
                root_uuid,
                "rebind",
                "planned",
                &"f".repeat(64),
                &serde_json::json!({"planUuid": "00000000-0000-7000-8000-000000000023"}),
                0,
                Some(1),
            )
            .unwrap_err();
        assert!(blocked.to_string().contains("comparison-required"));
        assert!(
            registry
                .begin_root_operation_once(
                    "00000000-0000-7000-8000-000000000024",
                    library_uuid,
                    root_uuid,
                    "re-enable",
                    "comparing",
                    &"1".repeat(64),
                    &serde_json::json!({"mode": "merge-local-fork"}),
                    0,
                    Some(1),
                )
                .unwrap()
                .1
        );
        let active_operations = registry
            .root_operations(library_uuid, root_uuid, None, 50, "active")
            .unwrap();
        assert_eq!(active_operations.len(), 1);
        assert_eq!(
            active_operations[0].operation_uuid,
            "00000000-0000-7000-8000-000000000024"
        );
        let first_page = registry
            .root_operations(library_uuid, root_uuid, None, 1, "all")
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(
            first_page[0].operation_uuid,
            "00000000-0000-7000-8000-000000000024"
        );
        let second_page = registry
            .root_operations(
                library_uuid,
                root_uuid,
                Some(&first_page[0].operation_uuid),
                1,
                "all",
            )
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].operation_uuid, operation_uuid);
        let terminal_operations = registry
            .root_operations(library_uuid, root_uuid, None, 50, "terminal")
            .unwrap();
        assert_eq!(terminal_operations.len(), 1);
        assert_eq!(terminal_operations[0].operation_uuid, operation_uuid);
        assert!(registry
            .root_operations(
                library_uuid,
                "00000000-0000-7000-8000-000000000099",
                None,
                50,
                "all",
            )
            .unwrap()
            .is_empty());

        drop(registry);
        let reopened = HostRegistry::open(&path).expect("reopen host registry");
        assert!(reopened
            .local_root_binding(library_uuid, root_uuid)
            .unwrap()
            .is_some());
        let connection = reopened.connection.lock().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT schema_version FROM host_meta WHERE singleton=1",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            18
        );
        for table in [
            "provider_remote_descriptor_heads",
            "provider_verified_descriptors",
        ] {
            assert!(connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap());
        }
        for table in ["provider_remote_libraries", "sync_restore_plans"] {
            for column in ["descriptor_key", "checkpoint_key", "checkpoint_sha256"] {
                assert!(connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
                        params![table, column],
                        |row| row.get::<_, bool>(0)
                    )
                    .unwrap());
            }
        }
        assert!(connection
            .execute(
                "UPDATE remote_sync_roots SET root_folder_uuid='00000000-0000-7000-8000-000000000008' WHERE library_uuid=?1 AND root_uuid=?2",
                params![library_uuid, root_uuid],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE root_operations SET request_hash=?2 WHERE operation_uuid=?1",
                params![operation_uuid, "f".repeat(64)],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM remote_sync_roots WHERE library_uuid=?1 AND root_uuid=?2",
                params![library_uuid, root_uuid],
            )
            .is_err());
        assert!(connection.execute(
            "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,created_at,updated_at) VALUES (?1,?2,'00000000-0000-7000-8000-000000000006','D:/Library/Photos/Nested','d:/library/photos/nested','D:/Library/Photos','d:/library/photos','active','Nested','nested',?3,1,1)",
            params![library_uuid,root_uuid,"b".repeat(64)],
        ).is_err());
        drop(connection);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn enrollment_binding_stages_applied_name_and_completes_after_catalog_projection() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        let library_uuid = "00000000-0000-7000-8000-000000000301";
        let root_uuid = "00000000-0000-7000-8000-000000000302";
        let root_folder_uuid = "00000000-0000-7000-8000-000000000303";
        let operation_uuid = "00000000-0000-7000-8000-000000000304";
        let binding_uuid = "00000000-0000-7000-8000-000000000305";
        register_test_library(&registry, library_uuid);
        registry
            .upsert_remote_sync_root(
                library_uuid,
                root_uuid,
                root_folder_uuid,
                "Existing",
                "Existing",
                &"a".repeat(64),
                0,
                "active",
                None,
                None,
            )
            .expect("store remote root");
        let request = serde_json::json!({
            "operationUuid": operation_uuid,
            "expectedOperationRevision": 0,
            "rootUuid": root_uuid,
            "rootFolderUuid": root_folder_uuid,
            "preflightToken": operation_uuid,
            "path": "D:/Existing",
            "requestedName": "Existing"
        });
        let request_hash = crate::sync_folders::request_hash(&request).unwrap();
        registry
            .store_root_path_preflight(
                operation_uuid,
                library_uuid,
                Some(root_uuid),
                "enroll-existing",
                &request_hash,
                &serde_json::json!({}),
                &serde_json::json!({}),
                unix_timestamp_millis() + 60_000,
            )
            .expect("store preflight");
        registry
            .begin_root_operation_once(
                operation_uuid,
                library_uuid,
                root_uuid,
                "enroll",
                "preflighting",
                &request_hash,
                &request,
                0,
                None,
            )
            .expect("begin enrollment");
        registry
            .set_root_operation_plan(
                operation_uuid,
                operation_uuid,
                &serde_json::json!({"preflightToken":operation_uuid}),
                "capturing-seed",
            )
            .expect("set enrollment plan");
        let binding = NewLocalRootBinding {
            library_uuid: library_uuid.into(),
            root_uuid: root_uuid.into(),
            local_binding_uuid: binding_uuid.into(),
            local_path: "D:/Existing".into(),
            canonical_path_key: "d:/existing".into(),
            placement_parent: "D:/".into(),
            placement_parent_key: "d:".into(),
            marker_identity: "marker".into(),
            filesystem_identity: Some("filesystem".into()),
            state: "active".into(),
            binding_generation: 1,
            root_capture_epoch: Some(Uuid::now_v7().to_string()),
            root_actor_epoch: Some(Uuid::now_v7().to_string()),
            desired_materialized_name: "Existing".into(),
            desired_materialization_fingerprint: "a".repeat(64),
            applied_materialized_name: Some("Existing".into()),
            applied_materialization_fingerprint: Some("a".repeat(64)),
            operation_revision: 1,
            verification_json: serde_json::json!({}),
        };

        let staged = registry
            .commit_local_root_binding(operation_uuid, operation_uuid, &binding)
            .expect("stage enrollment binding");
        assert_eq!(staged.state, "active");
        assert_eq!(
            staged.applied_materialized_name.as_deref(),
            Some("Existing")
        );
        assert_eq!(
            staged.applied_materialization_fingerprint.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        let operation = registry
            .root_operation(operation_uuid)
            .unwrap()
            .expect("staged enrollment operation");
        assert_eq!(
            (operation.state.as_str(), operation.phase.as_str()),
            ("waiting", "publishing-descriptor")
        );
        assert!(registry
            .pending_automatic_root_renames(library_uuid)
            .unwrap()
            .is_empty());

        registry
            .commit_local_root_binding(operation_uuid, operation_uuid, &binding)
            .expect("replay staged enrollment binding");
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE root_path_preflights SET expires_at=0 WHERE preflight_token=?1",
                [operation_uuid],
            )
            .unwrap();
        let completed = registry
            .complete_root_enrollment(operation_uuid, operation_uuid, operation_uuid)
            .expect("complete staged enrollment after catalog projection");
        assert_eq!(
            (completed.state.as_str(), completed.phase.as_str()),
            ("completed", "completed")
        );

        let preflight = registry
            .root_path_preflight(operation_uuid, library_uuid)
            .expect("read consumed preflight");
        assert!(preflight.6.is_some());

        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE local_root_bindings SET state='local-rename-required',applied_materialized_name=NULL,applied_materialized_name_key=NULL,applied_materialization_fingerprint=NULL,last_error_code='local-rename-required',operation_revision=operation_revision+1 WHERE library_uuid=?1 AND root_uuid=?2",
                params![library_uuid,root_uuid],
            )
            .expect("simulate legacy enrollment rename failure");
        let mut replay = binding.clone();
        replay.operation_revision = completed.operation_revision + 1;
        let repaired = registry
            .commit_local_root_binding(operation_uuid, operation_uuid, &replay)
            .expect("repair legacy completed enrollment");
        assert_eq!(repaired.state, "active");
        assert_eq!(repaired.last_error_code, None);
        assert_eq!(
            repaired.applied_materialization_fingerprint,
            Some("a".repeat(64))
        );
        assert!(registry
            .pending_automatic_root_renames(library_uuid)
            .unwrap()
            .is_empty());
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn filesystem_identity_lookup_detects_cross_library_aliases() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        let owner_library_uuid = "00000000-0000-7000-8000-000000000311";
        let other_library_uuid = "00000000-0000-7000-8000-000000000312";
        let root_uuid = "00000000-0000-7000-8000-000000000313";
        register_test_library(&registry, owner_library_uuid);
        register_test_library(&registry, other_library_uuid);
        registry
            .upsert_remote_sync_root(
                owner_library_uuid,
                root_uuid,
                "00000000-0000-7000-8000-000000000314",
                "Existing",
                "Existing",
                &"a".repeat(64),
                0,
                "active",
                None,
                None,
            )
            .expect("store remote root");
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,marker_identity,filesystem_identity,state,binding_generation,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,applied_materialized_name,applied_materialized_name_key,applied_materialization_fingerprint,operation_revision,verification_json,created_at,updated_at) VALUES (?1,?2,'00000000-0000-7000-8000-000000000315','D:/Owned','d:/owned','D:/','d:','marker','volume-file','active',1,'Existing','existing',?3,'Existing','existing',?3,1,'{}',1,1)",
                params![owner_library_uuid, root_uuid, "a".repeat(64)],
            )
            .expect("store local binding");

        let bindings = registry
            .local_root_bindings_for_filesystem_identity("volume-file")
            .expect("find filesystem owner");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].library_uuid, owner_library_uuid);
        assert_ne!(bindings[0].library_uuid, other_library_uuid);
        assert_eq!(bindings[0].root_uuid, root_uuid);

        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn relocation_journal_fences_only_one_binding_and_is_idempotent() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let library_uuid = "00000000-0000-7000-8000-000000000201";
        let root_uuid = "00000000-0000-7000-8000-000000000202";
        let root_folder_uuid = "00000000-0000-7000-8000-000000000203";
        let binding_uuid = "00000000-0000-7000-8000-000000000204";
        let operation_uuid = "00000000-0000-7000-8000-000000000205";
        register_test_library(&registry, library_uuid);
        registry
            .upsert_remote_sync_root(
                library_uuid,
                root_uuid,
                root_folder_uuid,
                "Photos",
                "Photos",
                &"a".repeat(64),
                1,
                "active",
                None,
                None,
            )
            .unwrap();
        let now = unix_timestamp_millis();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,binding_generation,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,applied_materialized_name,applied_materialized_name_key,applied_materialization_fingerprint,created_at,updated_at) VALUES (?1,?2,?3,'D:/Library/Photos','d:/library/photos','D:/Library','d:/library','paused',1,'Photos','photos',?4,'Photos','photos',?4,?5,?5)",
            params![library_uuid,root_uuid,binding_uuid,"a".repeat(64),now],
        ).unwrap();
        let evidence = crate::sync_folders::RelocationEvidence {
            source_path: "D:/Library/Photos".into(),
            source_canonical_key: "d:/library/photos".into(),
            source_filesystem_identity: "source".into(),
            source_digest: "b".repeat(64),
            source_bytes: 0,
            available_bytes: None,
            target_path: "D:/Library/Renamed".into(),
            target_canonical_key: "d:/library/renamed".into(),
            target_parent_identity: "parent".into(),
            same_volume: true,
        };
        let request = serde_json::json!({
            "localBindingUuid": binding_uuid,
            "pathEvidence": evidence,
            "resumeState": "paused",
        });
        let hash = crate::sync_folders::request_hash(&request).unwrap();
        let evidence: crate::sync_folders::RelocationEvidence =
            serde_json::from_value(request["pathEvidence"].clone()).unwrap();
        let (first, created) = registry
            .begin_root_relocation(
                operation_uuid,
                library_uuid,
                root_uuid,
                "explicit-move",
                &hash,
                &request,
                0,
                1,
                &"a".repeat(64),
                &evidence,
            )
            .unwrap();
        assert!(created);
        assert_eq!(first.operation.phase, "fenced");
        assert_eq!(
            registry
                .local_root_binding(library_uuid, root_uuid)
                .unwrap()
                .unwrap()
                .binding_generation,
            2
        );
        let (retry, created) = registry
            .begin_root_relocation(
                operation_uuid,
                library_uuid,
                root_uuid,
                "explicit-move",
                &hash,
                &request,
                0,
                1,
                &"a".repeat(64),
                &evidence,
            )
            .unwrap();
        assert!(!created);
        assert_eq!(retry.operation.operation_uuid, operation_uuid);
        assert_eq!(
            registry
                .advance_root_relocation(
                    operation_uuid,
                    0,
                    "transferring",
                    None,
                    None,
                    None,
                    0,
                    0,
                    false
                )
                .unwrap_err()
                .to_string(),
            "stale-operation-revision"
        );

        let failed = registry
            .fail_root_relocation(
                operation_uuid,
                first.operation.operation_revision,
                "transfer-failed",
                "transfer-failed",
            )
            .unwrap();
        assert_eq!(failed.operation_revision, 2);
        assert_eq!(
            registry
                .local_root_binding(library_uuid, root_uuid)
                .unwrap()
                .unwrap()
                .state,
            "paused"
        );
        let retried = registry
            .retry_root_relocation(operation_uuid, failed.operation_revision, 2)
            .unwrap();
        assert_eq!(retried.operation.state, "running");
        let idempotent_retry = registry
            .retry_root_relocation(operation_uuid, failed.operation_revision, 2)
            .unwrap();
        assert_eq!(idempotent_retry.operation.operation_revision, 3);
        let cancelled = registry
            .cancel_root_relocation(
                operation_uuid,
                idempotent_retry.operation.operation_revision,
                2,
            )
            .unwrap();
        assert_eq!(cancelled.state, "cancelled");
        let idempotent_cancel = registry
            .cancel_root_relocation(
                operation_uuid,
                idempotent_retry.operation.operation_revision,
                2,
            )
            .unwrap();
        assert_eq!(
            idempotent_cancel.operation_revision,
            cancelled.operation_revision
        );
        assert_eq!(
            registry
                .local_root_binding(library_uuid, root_uuid)
                .unwrap()
                .unwrap()
                .state,
            "paused"
        );

        let failure_operation_uuid = "00000000-0000-7000-8000-000000000206";
        let binding = registry
            .local_root_binding(library_uuid, root_uuid)
            .unwrap()
            .unwrap();
        let failure_request = serde_json::json!({
            "localBindingUuid": binding_uuid,
            "operationUuid": failure_operation_uuid,
            "resumeState": "paused",
            "targetPath": "D:/Library/Blocked",
        });
        let failure_hash = crate::sync_folders::request_hash(&failure_request).unwrap();
        let failure = registry
            .record_automatic_root_rename_preflight_failure(
                failure_operation_uuid,
                library_uuid,
                root_uuid,
                &failure_hash,
                &failure_request,
                binding.operation_revision,
                binding.binding_generation,
                &"c".repeat(64),
                Path::new("D:/Library/Blocked"),
            )
            .unwrap();
        assert_eq!(failure.state, "failed");
        assert_eq!(failure.phase, "local-rename-required");
        assert_eq!(failure.error_code.as_deref(), Some("local-rename-required"));
        let failed_binding = registry
            .local_root_binding(library_uuid, root_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(failed_binding.state, "local-rename-required");
        assert_eq!(
            failed_binding.last_error_code.as_deref(),
            Some("local-rename-required")
        );
        let duplicate = registry
            .record_automatic_root_rename_preflight_failure(
                failure_operation_uuid,
                library_uuid,
                root_uuid,
                &failure_hash,
                &failure_request,
                failed_binding.operation_revision,
                failed_binding.binding_generation,
                &"c".repeat(64),
                Path::new("D:/Library/Blocked"),
            )
            .unwrap();
        assert_eq!(duplicate.operation_uuid, failure.operation_uuid);
        assert_eq!(
            registry
                .local_root_binding(library_uuid, root_uuid)
                .unwrap()
                .unwrap()
                .operation_revision,
            failed_binding.operation_revision
        );
        drop(registry);
        let reopened = HostRegistry::open(&path).unwrap();
        assert_eq!(
            reopened
                .local_root_binding(library_uuid, root_uuid)
                .unwrap()
                .unwrap()
                .state,
            "local-rename-required"
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stop_sync_seals_selection_and_reenable_allocates_a_fresh_binding_generation() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        let library_uuid = "00000000-0000-7000-8000-000000000101";
        let root_uuid = "00000000-0000-7000-8000-000000000102";
        let root_folder_uuid = "00000000-0000-7000-8000-000000000103";
        let original_binding_uuid = "00000000-0000-7000-8000-000000000104";
        register_test_plugin(&registry, "com.megle.root-test", "account-root-test");
        register_test_library(&registry, library_uuid);
        registry
            .begin_sync_binding(library_uuid, "com.megle.root-test", "account-root-test")
            .unwrap();
        {
            let connection = registry.connection.lock().unwrap();
            connection.execute(
                "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?1,?2,?3,'Photos','Photos','photos',?4,'active','unselected',1)",
                params![library_uuid, root_uuid, root_folder_uuid, "a".repeat(64)],
            ).unwrap();
            connection.execute(
                "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,binding_generation,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,created_at,updated_at) VALUES (?1,?2,?3,'D:/Library/Photos','d:/library/photos','D:/Library','d:/library','active',1,'Photos','photos',?4,1,1)",
                params![library_uuid, root_uuid, original_binding_uuid, "a".repeat(64)],
            ).unwrap();
        }
        let snapshot = crate::sync_folders::RootSnapshot::from_entities(
            root_uuid.to_owned(),
            serde_json::json!({"actor": 7}),
            BTreeMap::new(),
        )
        .unwrap();
        let stop_request = serde_json::json!({"command":"stop-sync"});
        let stop_plan = registry
            .create_stop_sync_plan(
                "00000000-0000-7000-8000-000000000105",
                library_uuid,
                root_uuid,
                &"b".repeat(64),
                &stop_request,
                0,
                1,
                &snapshot,
                &crate::sync_folders::RootStopWork {
                    pending_work: 0,
                    published_work: 0,
                    receipt_keys: Vec::new(),
                },
            )
            .unwrap();
        let stale_work = registry
            .commit_stop_sync(
                "00000000-0000-7000-8000-000000000105",
                &stop_plan.plan_uuid,
                library_uuid,
                root_uuid,
                &"b".repeat(64),
                &stop_request,
                0,
                1,
                &snapshot,
                &crate::sync_folders::RootStopWork {
                    pending_work: 1,
                    published_work: 0,
                    receipt_keys: Vec::new(),
                },
            )
            .unwrap_err();
        assert!(stale_work.to_string().contains("stale-plan"));
        registry
            .commit_stop_sync(
                "00000000-0000-7000-8000-000000000105",
                &stop_plan.plan_uuid,
                library_uuid,
                root_uuid,
                &"b".repeat(64),
                &stop_request,
                0,
                1,
                &snapshot,
                &crate::sync_folders::RootStopWork {
                    pending_work: 0,
                    published_work: 0,
                    receipt_keys: Vec::new(),
                },
            )
            .unwrap();
        let stopped = registry
            .local_root_binding(library_uuid, root_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(stopped.state, "local-only-fork");
        assert_eq!(stopped.local_binding_uuid, original_binding_uuid);
        assert_eq!(stopped.binding_generation, 2);
        assert_eq!(stopped.operation_revision, 1);
        assert_ne!(
            stopped.root_capture_epoch.as_deref(),
            Some(original_binding_uuid)
        );
        assert_ne!(
            stopped.root_actor_epoch.as_deref(),
            Some(original_binding_uuid)
        );
        drop(registry);
        let registry = HostRegistry::open(&path).expect("reopen visibility journal");
        registry
            .retry_root_visibility_reconciliation(
                "00000000-0000-7000-8000-000000000105",
                "provider-temporarily-unavailable",
                0,
            )
            .unwrap();
        {
            let connection = registry.connection.lock().unwrap();
            let visibility: (String, String) = connection
                .query_row(
                    "SELECT visibility.state,operation.state FROM root_visibility_reconciliations visibility JOIN root_operations operation ON operation.operation_uuid=visibility.operation_uuid WHERE visibility.operation_uuid=?1",
                    ["00000000-0000-7000-8000-000000000105"],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(visibility, ("retryable-unknown".into(), "running".into()));
        }
        registry
            .finish_root_visibility_reconciliation(
                "00000000-0000-7000-8000-000000000105",
                "hidden",
                &serde_json::json!({"objects":[]}),
            )
            .unwrap();
        let sealed = registry
            .begin_root_operation_once(
                "00000000-0000-7000-8000-000000000106",
                library_uuid,
                root_uuid,
                "select",
                "planned",
                &"c".repeat(64),
                &serde_json::json!({"selected":true}),
                1,
                Some(2),
            )
            .unwrap_err();
        assert!(sealed.to_string().contains("comparison-required"));

        let comparison_operation_uuid = "00000000-0000-7000-8000-000000000107";
        let mut local_entities = BTreeMap::new();
        local_entities.insert(
            root_folder_uuid.to_owned(),
            BTreeMap::from([("root:requested-name".to_owned(), "Local Photos".to_owned())]),
        );
        let local_snapshot = crate::sync_folders::RootSnapshot::from_entities(
            root_uuid.to_owned(),
            serde_json::json!({"actor": 8}),
            local_entities,
        )
        .unwrap();
        let differences =
            crate::sync_folders::compare_snapshots(&snapshot, &local_snapshot, &snapshot);
        let plan = registry
            .create_comparison_plan(
                comparison_operation_uuid,
                library_uuid,
                root_uuid,
                &"d".repeat(64),
                &serde_json::json!({"mode":"merge-local-fork"}),
                1,
                2,
                &registry
                    .cutover_base_snapshot(library_uuid, root_uuid)
                    .unwrap()
                    .0,
                &snapshot,
                &local_snapshot,
                &snapshot,
                &differences,
                0,
                0,
            )
            .unwrap();
        let (_, rebound) = registry
            .commit_reenable(
                comparison_operation_uuid,
                library_uuid,
                root_uuid,
                &plan.plan_uuid,
                1,
                2,
                &local_snapshot.digest,
                &snapshot.digest,
                0,
                0,
                "merge-local-fork",
            )
            .unwrap();
        assert_eq!(rebound.state, "reconcile-required");
        assert_eq!(rebound.binding_generation, 3);
        assert_ne!(rebound.local_binding_uuid, original_binding_uuid);
        assert_ne!(rebound.root_capture_epoch, stopped.root_capture_epoch);
        assert_ne!(rebound.root_actor_epoch, stopped.root_actor_epoch);
        let work = registry
            .pending_root_reconciliation_work(library_uuid, root_uuid)
            .unwrap();
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].resolution, "keep-local");
        assert_eq!(work[0].selected_value.as_deref(), Some("Local Photos"));
        let (_, retried) = registry
            .commit_reenable(
                comparison_operation_uuid,
                library_uuid,
                root_uuid,
                &plan.plan_uuid,
                1,
                2,
                &local_snapshot.digest,
                &snapshot.digest,
                0,
                0,
                "merge-local-fork",
            )
            .unwrap();
        assert_eq!(retried.local_binding_uuid, rebound.local_binding_uuid);
        assert_eq!(retried.binding_generation, 3);
        registry
            .complete_root_reconciliation_item(&work[0].plan_uuid, work[0].item_ordinal)
            .unwrap();
        let completed = registry
            .complete_root_reconciliation(library_uuid, root_uuid, &plan.plan_uuid)
            .unwrap();
        assert_eq!(completed.state, "completed");
        assert_eq!(
            registry
                .local_root_binding(library_uuid, root_uuid)
                .unwrap()
                .unwrap()
                .state,
            "active"
        );

        drop(registry);
        let reopened = HostRegistry::open(&path).unwrap();
        let durable = reopened
            .local_root_binding(library_uuid, root_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(durable.local_binding_uuid, rebound.local_binding_uuid);
        assert_eq!(durable.binding_generation, 3);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn publish_as_new_allocates_a_distinct_enrollment_identity_and_is_recoverable() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let library_uuid = "00000000-0000-7000-8000-000000000151";
        let root_uuid = "00000000-0000-7000-8000-000000000152";
        register_test_plugin(&registry, "com.megle.publish-test", "account-publish-test");
        register_test_library(&registry, library_uuid);
        registry
            .begin_sync_binding(
                library_uuid,
                "com.megle.publish-test",
                "account-publish-test",
            )
            .unwrap();
        {
            let connection = registry.connection.lock().unwrap();
            connection.execute("INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?1,?2,'00000000-0000-7000-8000-000000000153','Photos','Photos','photos',?3,'active','unselected',1)",params![library_uuid,root_uuid,"a".repeat(64)]).unwrap();
            connection.execute("INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,binding_generation,root_capture_epoch,root_actor_epoch,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,operation_revision,created_at,updated_at) VALUES (?1,?2,'00000000-0000-7000-8000-000000000154','D:/Library/Photos','d:/library/photos','D:/Library','d:/library','local-only-fork',2,'00000000-0000-7000-8000-000000000155','00000000-0000-7000-8000-000000000156','Photos','photos',?3,1,1,1)",params![library_uuid,root_uuid,"a".repeat(64)]).unwrap();
            connection.execute("INSERT INTO root_cutover_snapshots(snapshot_uuid,operation_uuid,library_uuid,root_uuid,binding_generation,event_frontier_json,catalog_snapshot_digest,provider_receipts_json,remote_visibility,created_at) VALUES ('00000000-0000-7000-8000-000000000157','00000000-0000-7000-8000-000000000158',?1,?2,1,'{}',?3,'{}','hidden',1)",params![library_uuid,root_uuid,"b".repeat(64)]).unwrap_err();
        }
        let snapshot = crate::sync_folders::RootSnapshot::from_entities(
            root_uuid.to_owned(),
            serde_json::json!({}),
            BTreeMap::new(),
        )
        .unwrap();
        {
            let connection = registry.connection.lock().unwrap();
            connection.execute("INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,created_at,updated_at) VALUES ('00000000-0000-7000-8000-000000000158',?1,?2,'stop-sync','completed','completed',?3,'{}',0,1,1,1,1)",params![library_uuid,root_uuid,"b".repeat(64)]).unwrap();
            connection.execute("INSERT INTO root_cutover_snapshots(snapshot_uuid,operation_uuid,library_uuid,root_uuid,binding_generation,event_frontier_json,catalog_snapshot_digest,provider_receipts_json,remote_visibility,created_at) VALUES ('00000000-0000-7000-8000-000000000157','00000000-0000-7000-8000-000000000158',?1,?2,1,'{}',?3,?4,'hidden',1)",params![library_uuid,root_uuid,snapshot.digest,serde_json::to_string(&serde_json::json!({"baseSnapshot":snapshot})).unwrap()]).unwrap();
        }
        let operation_uuid = "00000000-0000-7000-8000-000000000159";
        let plan = registry
            .create_comparison_plan(
                operation_uuid,
                library_uuid,
                root_uuid,
                &"c".repeat(64),
                &serde_json::json!({"mode":"publish-as-new"}),
                1,
                2,
                "00000000-0000-7000-8000-000000000157",
                &snapshot,
                &snapshot,
                &snapshot,
                &[],
                0,
                0,
            )
            .unwrap();
        let (_, publish) = registry
            .commit_publish_as_new(
                operation_uuid,
                library_uuid,
                root_uuid,
                &plan.plan_uuid,
                1,
                2,
                &snapshot.digest,
                &snapshot.digest,
                0,
                0,
            )
            .unwrap();
        assert_ne!(publish.new_root_uuid, root_uuid);
        assert!(registry
            .local_root_binding(library_uuid, root_uuid)
            .unwrap()
            .is_none());
        let new_binding = registry
            .local_root_binding(library_uuid, &publish.new_root_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(new_binding.local_path, "D:/Library/Photos");
        assert_eq!(new_binding.state, "materializing");
        assert_ne!(
            new_binding.root_capture_epoch.as_deref(),
            Some("00000000-0000-7000-8000-000000000155")
        );
        drop(registry);
        let reopened = HostRegistry::open(&path).unwrap();
        let pending = reopened.pending_publish_as_new(library_uuid).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].new_root_uuid, publish.new_root_uuid);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn selection_aggregate_validates_deselections_before_creating_any_binding() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        let library_uuid = "00000000-0000-7000-8000-000000000201";
        let selected_root = "00000000-0000-7000-8000-000000000202";
        let deselected_root = "00000000-0000-7000-8000-000000000203";
        register_test_library(&registry, library_uuid);
        {
            let connection = registry.connection.lock().unwrap();
            for (root_uuid, folder_uuid, name) in [
                (
                    selected_root,
                    "00000000-0000-7000-8000-000000000204",
                    "Photos",
                ),
                (
                    deselected_root,
                    "00000000-0000-7000-8000-000000000205",
                    "Photos 2",
                ),
            ] {
                connection.execute(
                    "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?1,?2,?3,?4,?4,lower(?4),?5,'active','placement-required',1)",
                    params![library_uuid, root_uuid, folder_uuid, name, "a".repeat(64)],
                ).unwrap();
            }
        }
        let plan_uuid = "00000000-0000-7000-8000-000000000206";
        let operation_uuid = "00000000-0000-7000-8000-000000000207";
        registry
            .begin_root_operations_aggregate(
                library_uuid,
                plan_uuid,
                &[PlannedRootOperation {
                    operation_uuid: operation_uuid.into(),
                    root_uuid: selected_root.into(),
                    kind: "select".into(),
                    phase: "preflighting".into(),
                    request_hash: "b".repeat(64),
                    request: serde_json::json!({"selected": true}),
                    expected_operation_revision: 0,
                    expected_binding_generation: None,
                    result: serde_json::json!({}),
                }],
            )
            .unwrap();
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE remote_sync_roots SET operation_revision=1 WHERE library_uuid=?1 AND root_uuid=?2",
                params![library_uuid, deselected_root],
            )
            .unwrap();
        let binding = NewLocalRootBinding {
            library_uuid: library_uuid.into(),
            root_uuid: selected_root.into(),
            local_binding_uuid: "00000000-0000-7000-8000-000000000208".into(),
            local_path: "D:/Library/Photos".into(),
            canonical_path_key: "d:/library/photos".into(),
            placement_parent: "D:/Library".into(),
            placement_parent_key: "d:/library".into(),
            marker_identity: "marker".into(),
            filesystem_identity: Some("filesystem".into()),
            state: "materializing".into(),
            binding_generation: 1,
            root_capture_epoch: Some(Uuid::now_v7().to_string()),
            root_actor_epoch: Some(Uuid::now_v7().to_string()),
            desired_materialized_name: "Photos".into(),
            desired_materialization_fingerprint: "a".repeat(64),
            applied_materialized_name: None,
            applied_materialization_fingerprint: None,
            operation_revision: 1,
            verification_json: serde_json::json!({}),
        };
        assert!(registry
            .complete_root_operations_aggregate(
                library_uuid,
                plan_uuid,
                &[binding],
                &[(deselected_root.into(), 0)],
            )
            .is_err());
        assert!(registry
            .local_root_binding(library_uuid, selected_root)
            .unwrap()
            .is_none());

        drop(registry);
        let _ = std::fs::remove_file(path);
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
    fn retry_binding_uses_durable_seed_receipts_and_preserves_transport_work() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "fake", "account");
        register_test_library(&registry, "library");
        let (epoch, _) = registry
            .begin_sync_binding("library", "fake", "account")
            .unwrap();
        let revision = registry.snapshot().unwrap().context_revision;
        let checkpoint = uuid::Uuid::now_v7();
        let checkpoint_key = format!("checkpoints/{checkpoint}.checkpoint");

        registry
            .record_sync_turn_error("library", "test_failure")
            .unwrap();
        let initial = registry
            .binding_transport("library", "library")
            .unwrap()
            .unwrap();
        assert!(registry
            .retry_binding("library", revision, &initial)
            .unwrap());
        assert_eq!(
            registry
                .binding_transport("library", "library")
                .unwrap()
                .unwrap()
                .state,
            "preparing"
        );
        registry
            .set_sync_capture_generation("library", epoch, 7)
            .unwrap();
        registry
            .mark_local_seed_ready("library", checkpoint, 7, true)
            .unwrap();
        registry
            .commit_binding_cursor(
                "library",
                epoch,
                1,
                "library",
                "library",
                Some("retained-cursor"),
            )
            .unwrap();
        registry.connection.lock().unwrap().execute(
            "UPDATE library_sync_bindings SET pending_upload_items=48,pending_upload_bytes=1091197953,published_through_seq=19 WHERE library_uuid='library'",
            [],
        ).unwrap();

        for (receipt, phase) in [
            (None, "seed_ready"),
            (Some("library-descriptor"), "seed_ready"),
            (Some(checkpoint_key.as_str()), "active"),
        ] {
            if let Some(key) = receipt {
                registry
                    .record_provider_receipt("library", epoch, 1, key, None, &[4; 32], "created")
                    .unwrap();
            }
            registry
                .record_sync_turn_error("library", "test_failure")
                .unwrap();
            let before = registry
                .binding_transport("library", "library")
                .unwrap()
                .unwrap();
            let status_before = registry.sync_status_snapshot().unwrap().libraries.remove(0);
            assert_eq!(
                status_before.last_error_code.as_deref(),
                Some("test_failure")
            );
            assert!(!registry
                .retry_binding("library", revision, &before)
                .unwrap());
            let after = registry
                .binding_transport("library", "library")
                .unwrap()
                .unwrap();
            assert_eq!(after.state, phase);
            assert_eq!(after.binding_epoch, before.binding_epoch);
            assert_eq!(after.actor_epoch, before.actor_epoch);
            assert_eq!(after.transport_generation, before.transport_generation);
            assert_eq!(after.cursor, before.cursor);
            assert_eq!(after.latest_checkpoint_uuid, before.latest_checkpoint_uuid);
            let status_after = registry.sync_status_snapshot().unwrap().libraries.remove(0);
            assert!(status_after.last_error_code.is_none());
            assert!(status_after.retry_after.is_none());
            assert_eq!(status_after.poll_after, Some(0));
            assert_eq!(status_after.pending_upload_items, 48);
            assert_eq!(status_after.pending_upload_bytes, 1091197953);
            let preserved: (i64, i64) = registry.connection.lock().unwrap().query_row(
                "SELECT capture_generation,published_through_seq FROM library_sync_bindings WHERE library_uuid='library'",
                [], |row| Ok((row.get(0)?,row.get(1)?)),
            ).unwrap();
            assert_eq!(preserved, (7, 19));
        }
        // A completed pause/resume cannot erase the original publication
        // facts or make retry rebuild its immutable descriptor.
        registry.pause_binding("library").unwrap();
        registry.set_binding_state("library", "active").unwrap();
        registry
            .record_sync_turn_error("library", "after_resume")
            .unwrap();
        let resumed = registry
            .binding_transport("library", "library")
            .unwrap()
            .unwrap();
        assert_eq!(resumed.transport_generation, 2);
        assert!(!registry
            .retry_binding("library", revision, &resumed)
            .unwrap());
        assert_eq!(
            registry
                .binding_transport("library", "library")
                .unwrap()
                .unwrap()
                .state,
            "active"
        );
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn retry_binding_rejects_stale_fenced_and_unready_states_without_erasing_error() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "fake", "account");
        register_test_library(&registry, "library");
        registry
            .begin_sync_binding("library", "fake", "account")
            .unwrap();
        registry
            .record_sync_turn_error("library", "original_failure")
            .unwrap();
        let expected = registry
            .binding_transport("library", "library")
            .unwrap()
            .unwrap();
        let revision = registry.snapshot().unwrap().context_revision;
        let before = registry.sync_status_snapshot().unwrap().libraries.remove(0);
        assert!(registry.wake_binding("library").is_err());
        assert_eq!(registry.wake_all_sync_bindings().unwrap(), 0);
        registry.record_sync_turn_success("library", true).unwrap();
        registry
            .record_sync_turn_error("library", "late_failure")
            .unwrap();
        let after = registry.sync_status_snapshot().unwrap().libraries.remove(0);
        assert_eq!(after.last_error_code, before.last_error_code);
        assert_eq!(after.retry_after, before.retry_after);
        assert_eq!(after.last_success_at, before.last_success_at);
        assert_eq!(
            registry
                .retry_binding("library", revision + 1, &expected)
                .unwrap_err()
                .to_string(),
            "stale-library-context"
        );
        let mut stale = expected.clone();
        stale.transport_generation += 1;
        assert_eq!(
            registry
                .retry_binding("library", revision, &stale)
                .unwrap_err()
                .to_string(),
            "stale-provider-transport-generation"
        );
        registry
            .update_plugin_account_auth("fake", "expired", None)
            .unwrap();
        assert_eq!(
            registry
                .retry_binding("library", revision, &expected)
                .unwrap_err()
                .to_string(),
            "provider-not-ready"
        );
        registry
            .update_plugin_account_auth("fake", "authenticated", None)
            .unwrap();
        for state in [
            "paused",
            "unbinding",
            "compatibility_locked",
            "active",
            "restoring",
        ] {
            registry.set_binding_state("library", state).unwrap();
            let expected = registry
                .binding_transport("library", "library")
                .unwrap()
                .unwrap();
            assert_eq!(
                registry
                    .retry_binding("library", revision, &expected)
                    .unwrap_err()
                    .to_string(),
                "sync-action-not-allowed"
            );
            if matches!(state, "paused" | "unbinding" | "compatibility_locked") {
                registry
                    .record_sync_turn_error("library", "late_failure")
                    .unwrap();
                registry.record_sync_turn_success("library", true).unwrap();
                assert!(registry.wake_binding("library").is_err());
            }
            assert_eq!(
                registry
                    .binding_transport("library", "library")
                    .unwrap()
                    .unwrap()
                    .state,
                state
            );
        }
        drop(registry);
        let _ = std::fs::remove_file(path);
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
    fn host_plugin_name_comes_from_the_selected_version_manifest() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.named",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2,"name":"Named Provider v1"}"#,
                entry_relative_path: "bin/provider-v1",
                entry_sha256: &[7; 32],
                install_root: Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.named",
                version: "2.0.0",
                manifest_json: r#"{"manifestVersion":2,"name":"Named Provider v2"}"#,
                entry_relative_path: "bin/provider-v2",
                entry_sha256: &[8; 32],
                install_root: Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE plugin_installations SET selected_version='2.0.0' WHERE plugin_id='com.megle.named'",
                [],
            )
            .unwrap();

        let listed = registry.list_host_plugins().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Named Provider v2");
        assert_eq!(
            registry
                .host_plugin("com.megle.named")
                .unwrap()
                .unwrap()
                .name,
            "Named Provider v2"
        );

        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn host_plugin_name_falls_back_for_a_historical_manifest_without_name() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.historical",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2,"name":"   "}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[7; 32],
                install_root: Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();

        let plugin = registry
            .host_plugin("com.megle.historical")
            .unwrap()
            .unwrap();
        assert_eq!(plugin.name, "com.megle.historical");
        assert_eq!(
            selected_plugin_name(&"p".repeat(255), Some(r#"{"name":""}"#)).len(),
            128
        );
        assert_eq!(
            selected_plugin_name(
                "com.megle.historical",
                Some(&format!(r#"{{"name":"{}"}}"#, "n".repeat(129)))
            ),
            "com.megle.historical"
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
    fn rediscovery_of_selected_embedded_plugin_updates_install_root() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let release_root = Path::new(r"D:\Megle\target\release\bundled-plugins\com.megle.onedrive");
        let debug_root = Path::new(r"D:\Megle\target\debug\bundled-plugins\com.megle.onedrive");
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.onedrive",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[7; 32],
                install_root: release_root,
                trust_policy_id: "embedded-v1",
                requested_permissions_json: "[]",
            })
            .unwrap();

        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.onedrive",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[8; 32],
                install_root: debug_root,
                trust_policy_id: "embedded-v1",
                requested_permissions_json: "[]",
            })
            .unwrap();

        let stored_root: String = registry
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT install_root FROM plugin_installations WHERE plugin_id='com.megle.onedrive'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(Path::new(&stored_root), debug_root);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn non_embedded_plugin_cannot_change_install_root_on_rediscovery() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.test",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[7; 32],
                install_root: Path::new("bundled-a"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();

        let error = registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.test",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[8; 32],
                install_root: Path::new("bundled-b"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "plugin-install-root-changed");
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
        let preflight = registry
            .create_restore_preflight(&request, 1, 42, &[], &[])
            .unwrap();
        let stored_result: String = registry
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT result_json FROM sync_restore_preflights WHERE token=?1",
                [preflight.preflight_token.as_deref().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stored_result).unwrap()["totalBytes"],
            42
        );
        assert!(registry
            .consume_restore_preflight(
                preflight.preflight_token.as_deref().unwrap(),
                &serde_json::json!({"items":[]})
            )
            .is_err());
        registry
            .consume_restore_preflight(preflight.preflight_token.as_deref().unwrap(), &request)
            .unwrap();
        assert!(registry
            .consume_restore_preflight(preflight.preflight_token.as_deref().unwrap(), &request)
            .is_err());
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restore_claim_records_all_plans_atomically_and_replays_exact_request() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        register_test_plugin(&registry, "fake", "account");
        for library_uuid in ["remote-a", "remote-b"] {
            registry.connection.lock().unwrap().execute(
                "INSERT INTO provider_remote_libraries(account_uuid,library_uuid,descriptor_size,descriptor_sha256,account_generation,updated_at,name,compatible) VALUES ('account',?1,1,zeroblob(32),1,?2,?1,1)",
                params![library_uuid, now],
            ).unwrap();
        }
        let request =
            serde_json::json!({"items":[{"libraryUuid":"remote-a"},{"libraryUuid":"remote-b"}]});
        let preflight = registry
            .create_restore_preflight(&request, 2, 0, &[], &[])
            .unwrap();
        let operation_uuid = uuid::Uuid::now_v7().to_string();
        let plans = ["remote-a", "remote-b"].map(|library_uuid| RestorePlanInput {
            account_uuid: "account".into(),
            library_uuid: library_uuid.into(),
            managed_root: PathBuf::from(format!("D:/{library_uuid}")),
            restore_mode: "legacy-library".into(),
            inventory_fence: None,
            roots: Vec::new(),
        });
        let (inserted, recorded) = registry
            .claim_restore_and_record_plans(
                preflight.preflight_token.as_deref().unwrap(),
                &operation_uuid,
                &request,
                &plans,
            )
            .unwrap();
        assert!(inserted);
        assert_eq!(recorded.len(), 2);
        let replayed = registry
            .replay_restore_operation(
                preflight.preflight_token.as_deref().unwrap(),
                &operation_uuid,
                &request,
            )
            .unwrap()
            .unwrap();
        assert_eq!(replayed.len(), 2);
        assert!(registry
            .replay_restore_operation(
                preflight.preflight_token.as_deref().unwrap(),
                &operation_uuid,
                &serde_json::json!({"items":[]}),
            )
            .is_err());
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restore_root_staging_preserves_explicit_deselection_without_placement_prompt() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library','library-root','Library','local','D:/Library','ready','local-only','available',1,'library-marker',?1,?1)",
            [now],
        ).unwrap();
        let selected = NewLocalRootBinding {
            library_uuid: "library".into(),
            root_uuid: "00000000-0000-7000-8000-000000000201".into(),
            local_binding_uuid: uuid::Uuid::now_v7().to_string(),
            local_path: "D:/Library/Selected".into(),
            canonical_path_key: "d:/library/selected".into(),
            placement_parent: "D:/Library".into(),
            placement_parent_key: "d:/library".into(),
            marker_identity: "selected-marker".into(),
            filesystem_identity: Some("selected-directory".into()),
            state: "materializing".into(),
            binding_generation: 1,
            root_capture_epoch: Some(uuid::Uuid::now_v7().to_string()),
            root_actor_epoch: Some(uuid::Uuid::now_v7().to_string()),
            desired_materialized_name: "Selected".into(),
            desired_materialization_fingerprint: "a".repeat(64),
            applied_materialized_name: None,
            applied_materialization_fingerprint: None,
            operation_revision: 0,
            verification_json: serde_json::json!({}),
        };
        let roots = [
            RestoreRootPlanInput {
                root_uuid: "00000000-0000-7000-8000-000000000201".into(),
                root_folder_uuid: uuid::Uuid::now_v7().to_string(),
                requested_name: "Selected".into(),
                materialized_name: "Selected".into(),
                materialization_fingerprint: "a".repeat(64),
                logical_revision: 1,
                logical_bytes: Some(10),
                selected_binding: Some(selected),
            },
            RestoreRootPlanInput {
                root_uuid: "00000000-0000-7000-8000-000000000202".into(),
                root_folder_uuid: uuid::Uuid::now_v7().to_string(),
                requested_name: "Deselected".into(),
                materialized_name: "Deselected".into(),
                materialization_fingerprint: "b".repeat(64),
                logical_revision: 2,
                logical_bytes: Some(20),
                selected_binding: None,
            },
        ];
        registry
            .import_restore_root_inventory("restore", "library", &roots)
            .unwrap();
        let connection = registry.connection.lock().unwrap();
        let states = connection
            .prepare("SELECT root_uuid,inventory_state FROM remote_sync_roots WHERE library_uuid='library' ORDER BY root_uuid")
            .unwrap()
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            states,
            vec![
                (
                    "00000000-0000-7000-8000-000000000201".into(),
                    "unselected".into()
                ),
                (
                    "00000000-0000-7000-8000-000000000202".into(),
                    "unselected".into()
                ),
            ]
        );
        let binding_count_before_stage: i64 = connection
            .query_row("SELECT COUNT(*) FROM local_root_bindings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(binding_count_before_stage, 0);
        drop(connection);
        registry
            .stage_restore_root_bindings("restore", "library", &roots)
            .unwrap();
        let binding_count: i64 = registry
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM local_root_bindings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(binding_count, 1);
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
                managed_root: Path::new("D:/Remote"),
                restore_mode: "legacy-library",
                inventory_fence: None,
                request_hash: "0000000000000000000000000000000000000000000000000000000000000000",
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
            evidence: None,
            history: BTreeMap::new(),
            metadata,
            name: Some("Remote library".into()),
            icon: Some("folder".into()),
            media_count: Some(12),
            logical_bytes: Some(42_000),
            last_synced_at: Some(1234),
            compatible: true,
            error_code: None,
            roots: vec![
                DiscoveredRemoteRoot {
                    root_uuid: "00000000-0000-7000-8000-000000000002".into(),
                    root_folder_uuid: "00000000-0000-7000-8000-000000000003".into(),
                    requested_name: "Photos".into(),
                    materialized_name: "Photos".into(),
                    materialization_fingerprint: "a".repeat(64),
                    logical_revision: 7,
                    logical_bytes: Some(41_000),
                },
                DiscoveredRemoteRoot {
                    root_uuid: "00000000-0000-7000-8000-000000000004".into(),
                    root_folder_uuid: "00000000-0000-7000-8000-000000000005".into(),
                    requested_name: "Videos".into(),
                    materialized_name: "Videos".into(),
                    materialization_fingerprint: "b".repeat(64),
                    logical_revision: 8,
                    logical_bytes: Some(1_000),
                },
            ],
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
                [&library_uuid],
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
        let first_inventory = registry
            .remote_library_root_inventory("account", &library_uuid, None, 1)
            .unwrap();
        let second_inventory = registry
            .remote_library_root_inventory(
                "account",
                &library_uuid,
                Some("00000000-0000-7000-8000-000000000002"),
                1,
            )
            .unwrap();
        assert_eq!(first_inventory.items.len(), 1);
        assert_eq!(
            first_inventory.items[0].root_uuid,
            "00000000-0000-7000-8000-000000000002"
        );
        assert_eq!(first_inventory.items[0].logical_bytes, Some(41_000));
        assert_eq!(second_inventory.items.len(), 1);
        assert_eq!(
            second_inventory.items[0].root_uuid,
            "00000000-0000-7000-8000-000000000004"
        );
        assert_eq!(
            first_inventory.inventory_fence,
            second_inventory.inventory_fence
        );
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE provider_remote_sync_roots SET logical_revision=logical_revision+1 WHERE account_uuid='account' AND library_uuid=?1 AND root_uuid='00000000-0000-7000-8000-000000000004'",
                [&library_uuid],
            )
            .unwrap();
        let changed_inventory = registry
            .remote_library_root_inventory("account", &library_uuid, None, 1)
            .unwrap();
        assert_ne!(
            first_inventory.inventory_fence,
            changed_inventory.inventory_fence
        );
        assert!(registry
            .remote_library_root_inventory("other-account", &library_uuid, None, 50)
            .is_err());
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
            wire_version: crate::sync::codec::WIRE_VERSION,
            envelope_version: crate::sync::codec::ENVELOPE_VERSION,
            reducer_version: crate::sync::codec::REDUCER_VERSION,
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
                wire_version: crate::sync::codec::WIRE_VERSION,
                envelope_version: crate::sync::codec::ENVELOPE_VERSION,
                reducer_version: crate::sync::codec::REDUCER_VERSION,
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
            wire_version: crate::sync::codec::WIRE_VERSION,
            envelope_version: crate::sync::codec::ENVELOPE_VERSION,
            reducer_version: crate::sync::codec::REDUCER_VERSION,
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
            r#"{{"manifestVersion":2,"id":"com.megle.versioned","name":"Versioned","version":"2.0.0","engine":"process","entry":{{"windowsX64":"provider-v2.exe","linuxX64":"provider-v2"}},"rpc":{{"wireVersions":[1],"capabilities":["sync-provider.v1"],"methodVersions":{{"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.poll":1,"auth.cancel":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1}}}},"activationEvents":["sync-enabled"],"requestedPermissions":[],"integrity":{{"sha256":"{}"}}}}"#,
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
            r#"{{"manifestVersion":2,"id":"com.megle.stale-probation","name":"Versioned","version":"2.0.0","engine":"process","entry":{{"windowsX64":"provider-v2.exe","linuxX64":"provider-v2"}},"rpc":{{"wireVersions":[1],"capabilities":["sync-provider.v1"],"methodVersions":{{"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.poll":1,"auth.cancel":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1}}}},"activationEvents":["sync-enabled"],"requestedPermissions":[],"integrity":{{"sha256":"{}"}}}}"#,
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

    #[test]
    fn onedrive_configuration_plan_commit_is_revisioned_and_noop_safe() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let initial = registry.onedrive_configuration().unwrap();
        assert_eq!(initial.source, "missing");
        assert!(!initial.configured);
        assert!(!initial.read_only);
        assert_eq!(
            initial.error_code.as_deref(),
            Some("onedrive-client-id-missing")
        );
        assert_eq!(registry.effective_onedrive_client_id().unwrap(), None);
        let plan = registry
            .plan_onedrive_configuration(
                Some("01234567-89ab-4cde-8fab-0123456789ab"),
                initial.revision,
            )
            .unwrap();
        assert!(plan.effective_changed);
        assert!(!plan.requires_confirmation);
        assert!(registry
            .commit_onedrive_configuration(&plan.plan_uuid, plan.expected_revision, false)
            .unwrap()
            .fences
            .is_empty());
        let configured = registry.onedrive_configuration().unwrap();
        assert_eq!(configured.source, "host-override");
        assert!(configured.configured);
        assert_eq!(configured.revision, initial.revision + 1);
        let no_op = registry
            .plan_onedrive_configuration(
                Some("01234567-89AB-4CDE-8FAB-0123456789AB"),
                configured.revision,
            )
            .unwrap();
        assert!(!no_op.effective_changed);
        registry
            .commit_onedrive_configuration(&no_op.plan_uuid, no_op.expected_revision, false)
            .unwrap();
        assert_eq!(
            registry.onedrive_configuration().unwrap().revision,
            configured.revision
        );
        assert!(registry
            .plan_onedrive_configuration(Some("not-a-uuid"), configured.revision)
            .is_err());
        drop(registry);

        let reopened = HostRegistry::open(&path).unwrap();
        let persisted = reopened.onedrive_configuration().unwrap();
        assert_eq!(persisted.source, "host-override");
        assert!(persisted.configured);
        assert_eq!(
            reopened.effective_onedrive_client_id().unwrap().as_deref(),
            Some("01234567-89ab-4cde-8fab-0123456789ab")
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn onedrive_configuration_change_journals_old_credential_until_deleted() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.onedrive", "onedrive-account");
        registry
            .set_plugin_account_secret("com.megle.onedrive", Some("old-secret"), "authenticated", 1)
            .unwrap();
        let initial = registry.onedrive_configuration().unwrap();
        let plan = registry
            .plan_onedrive_configuration(
                Some("01234567-89ab-4cde-8fab-0123456789ab"),
                initial.revision,
            )
            .unwrap();
        let commit = registry
            .commit_onedrive_configuration(&plan.plan_uuid, plan.expected_revision, true)
            .unwrap();
        assert!(commit.effective_changed);
        assert_eq!(
            registry
                .plugin_account_secret("com.megle.onedrive")
                .unwrap()
                .unwrap()
                .secret_ref,
            None
        );
        let due = registry.due_credential_retirements(10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].secret_ref, "old-secret");

        drop(registry);
        let reopened = HostRegistry::open(&path).unwrap();
        let due = reopened.due_credential_retirements(10).unwrap();
        assert_eq!(due.len(), 1, "retirement must survive restart");
        reopened
            .complete_credential_retirement(&due[0].retirement_uuid)
            .unwrap();
        assert!(reopened.due_credential_retirements(10).unwrap().is_empty());
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sign_out_journals_all_registered_scoped_secrets_across_restart() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.onedrive", "onedrive-account");
        registry
            .set_plugin_account_secret(
                "com.megle.onedrive",
                Some("oauth-secret"),
                "authenticated",
                1,
            )
            .unwrap();
        let fingerprint = registry
            .plugin_runtime_launches()
            .unwrap()
            .into_iter()
            .find(|launch| launch.plugin_id == "com.megle.onedrive")
            .unwrap()
            .fingerprint();
        assert!(registry
            .register_plugin_secret_handle_for_launch(
                "com.megle.onedrive",
                "onedrive-account",
                &fingerprint,
                "upload-secret",
                ONEDRIVE_SECRET_NAMESPACE,
                "upload-session-v1",
            )
            .unwrap());
        registry
            .sign_out_plugin_account("com.megle.onedrive", 1)
            .unwrap();
        let mut refs = registry
            .due_credential_retirements(10)
            .unwrap()
            .into_iter()
            .map(|entry| (entry.secret_ref, entry.purpose))
            .collect::<Vec<_>>();
        refs.sort();
        assert_eq!(
            refs,
            vec![
                ("oauth-secret".into(), ONEDRIVE_CREDENTIAL_PURPOSE.into()),
                ("upload-secret".into(), "upload-session-v1".into()),
            ]
        );
        drop(registry);
        let reopened = HostRegistry::open(&path).unwrap();
        assert_eq!(reopened.due_credential_retirements(10).unwrap().len(), 2);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn configuration_change_and_account_switch_journal_nested_secrets() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.onedrive", "onedrive-account");
        registry
            .set_plugin_account_secret("com.megle.onedrive", Some("old-oauth"), "authenticated", 1)
            .unwrap();
        let fingerprint = registry.plugin_runtime_launches().unwrap()[0].fingerprint();
        assert!(registry
            .register_plugin_secret_handle_for_launch(
                "com.megle.onedrive",
                "onedrive-account",
                &fingerprint,
                "old-upload",
                ONEDRIVE_SECRET_NAMESPACE,
                "upload-session-v1",
            )
            .unwrap());
        assert!(registry
            .set_plugin_account_secret_for_launch(
                "com.megle.onedrive",
                &fingerprint,
                Some("new-oauth"),
                "authenticated",
            )
            .unwrap());
        let mut switched = registry
            .due_credential_retirements(10)
            .unwrap()
            .into_iter()
            .map(|entry| entry.secret_ref)
            .collect::<Vec<_>>();
        switched.sort();
        assert_eq!(switched, vec!["old-oauth", "old-upload"]);

        for retirement in registry.due_credential_retirements(10).unwrap() {
            registry
                .complete_credential_retirement(&retirement.retirement_uuid)
                .unwrap();
        }
        let initial = registry.onedrive_configuration().unwrap();
        let plan = registry
            .plan_onedrive_configuration(
                Some("01234567-89ab-4cde-8fab-0123456789ab"),
                initial.revision,
            )
            .unwrap();
        registry
            .commit_onedrive_configuration(&plan.plan_uuid, plan.expected_revision, true)
            .unwrap();
        let configured = registry.due_credential_retirements(10).unwrap();
        assert_eq!(configured.len(), 1);
        assert_eq!(configured[0].secret_ref, "new-oauth");
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cancelling_switch_auth_restores_authenticated_projection() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.onedrive", "onedrive-account");
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE plugin_runtime_state SET session_id='session-a' WHERE plugin_id='com.megle.onedrive'",
                [],
            )
            .unwrap();
        assert!(registry
            .begin_plugin_auth_flow("com.megle.onedrive", "flow-a", "session-a")
            .unwrap());
        assert!(registry
            .claim_plugin_auth_cancellation("com.megle.onedrive", "flow-a", "session-a",)
            .unwrap());
        assert!(!registry
            .plugin_auth_flow_is_current("com.megle.onedrive", "flow-a", "session-a")
            .unwrap());
        assert!(!registry
            .plugin_auth_secret_write_is_allowed("com.megle.onedrive")
            .unwrap());
        assert!(registry
            .apply_plugin_auth_result_for_flow(
                "com.megle.onedrive",
                "flow-a",
                "session-a",
                "authenticated",
                Some("late account"),
                Some("business"),
                Some("late-drive"),
            )
            .unwrap()
            .is_none());
        assert_eq!(
            registry
                .host_plugin("com.megle.onedrive")
                .unwrap()
                .unwrap()
                .auth_state
                .as_deref(),
            Some("authorizing")
        );
        assert!(registry
            .cancel_plugin_auth_flow("com.megle.onedrive", "flow-a", "session-a")
            .unwrap());
        assert_eq!(
            registry
                .host_plugin("com.megle.onedrive")
                .unwrap()
                .unwrap()
                .auth_state
                .as_deref(),
            Some("authenticated")
        );
        assert!(!registry
            .cancel_plugin_auth_flow("com.megle.onedrive", "flow-a", "session-a")
            .unwrap());
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cancelled_auth_retires_a_credential_created_before_worker_join() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.onedrive", "onedrive-account");
        registry
            .set_plugin_account_secret("com.megle.onedrive", Some("old-secret"), "authenticated", 1)
            .unwrap();
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE plugin_runtime_state SET session_id='session-race' WHERE plugin_id='com.megle.onedrive'",
                [],
            )
            .unwrap();
        assert!(registry
            .begin_plugin_auth_flow("com.megle.onedrive", "flow-race", "session-race")
            .unwrap());
        assert!(registry
            .claim_plugin_auth_cancellation("com.megle.onedrive", "flow-race", "session-race",)
            .unwrap());
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE plugin_accounts SET secret_ref='late-secret',auth_state='authenticated' WHERE plugin_id='com.megle.onedrive'",
                [],
            )
            .unwrap();
        assert!(registry
            .cancel_plugin_auth_flow("com.megle.onedrive", "flow-race", "session-race",)
            .unwrap());
        let account = registry
            .plugin_account_secret("com.megle.onedrive")
            .unwrap()
            .unwrap();
        assert_eq!(account.secret_ref.as_deref(), Some("old-secret"));
        let due = registry.due_credential_retirements(10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].secret_ref, "late-secret");
        drop(registry);
        let _ = std::fs::remove_file(path);
    }
}
