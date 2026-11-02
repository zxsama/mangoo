//! Host-owned revisioned non-secret Provider configuration.
use crate::host_registry::{
    enqueue_plugin_secret_retirements, pause_plugin_bindings_in_transaction, unix_timestamp_millis,
    BindingFence, HostRegistry,
};
use crate::plugins::contributions::{PluginConfigurationField, PluginContributions};
use rusqlite::{params, OptionalExtension};
use std::collections::BTreeMap;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfiguration {
    pub source: String,
    pub configured: bool,
    pub read_only: bool,
    pub revision: u64,
    pub error_code: Option<String>,
    pub values: BTreeMap<String, String>,
    pub fields: Vec<PluginConfigurationField>,
    pub locked_fields: Vec<String>,
}
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfigurationPlan {
    pub plan_uuid: String,
    pub expected_revision: u64,
    pub effective_changed: bool,
    pub affected_libraries: u64,
    pub requires_confirmation: bool,
    pub expires_at: i64,
}
#[derive(Debug)]
pub struct PluginConfigurationCommit {
    pub fences: Vec<BindingFence>,
    pub effective_changed: bool,
}

pub(crate) fn declarations(
    connection: &rusqlite::Connection,
    plugin_id: &str,
) -> anyhow::Result<PluginContributions> {
    let value: Option<String> = connection.query_row("SELECT json_extract(v.manifest_json,'$.contributions') FROM plugin_versions v JOIN plugin_installations i ON i.plugin_id=v.plugin_id AND i.selected_version=v.version WHERE i.plugin_id=?1",[plugin_id],|r| r.get(0)).optional()?.flatten();
    value
        .map(|v| serde_json::from_str(&v).map_err(Into::into))
        .unwrap_or_else(|| Ok(PluginContributions::default()))
}

fn project(
    declaration: &PluginContributions,
    stored: &BTreeMap<String, String>,
    revision: u64,
) -> PluginConfiguration {
    let mut values = BTreeMap::new();
    let mut locked_fields = Vec::new();
    let mut configured = true;
    let mut sources = Vec::new();
    let mut error_code = None;
    let fields: Vec<_> = declaration
        .configuration
        .iter()
        .filter(|f| !f.debug_only || cfg!(debug_assertions))
        .cloned()
        .collect();
    for field in &fields {
        let environment = field
            .environment
            .as_ref()
            .and_then(|key| std::env::var(key).ok())
            .filter(|v| !v.trim().is_empty());
        if environment.is_some() {
            locked_fields.push(field.id.clone());
        }
        let raw = environment
            .as_ref()
            .or_else(|| stored.get(&field.id))
            .or(field.default_value.as_ref());
        if !field.hidden {
            sources.push(if environment.is_some() {
                "environment"
            } else if stored.contains_key(&field.id) {
                "host-override"
            } else if field.default_value.is_some() {
                "default"
            } else {
                "missing"
            });
        }
        match field.normalize(raw.map(String::as_str)) {
            Ok(Some(value)) => {
                values.insert(field.id.clone(), value);
            }
            Ok(None) if !field.required => {}
            Ok(None) => {
                configured = false;
                error_code = Some("plugin-configuration-required".into());
            }
            Err(_) => {
                configured = false;
                error_code = Some("invalid-plugin-configuration".into());
            }
        }
    }
    let source = ["missing", "environment", "host-override", "default"]
        .into_iter()
        .find(|s| sources.contains(s))
        .unwrap_or("default");
    let read_only = !sources.is_empty()
        && fields
            .iter()
            .filter(|f| !f.hidden)
            .all(|f| locked_fields.contains(&f.id));
    PluginConfiguration {
        source: source.into(),
        configured,
        read_only,
        revision,
        error_code,
        values,
        fields,
        locked_fields,
    }
}

fn stored(
    connection: &rusqlite::Connection,
    plugin_id: &str,
) -> anyhow::Result<(BTreeMap<String, String>, u64)> {
    let row: Option<(String, u64)> = connection
        .query_row(
            "SELECT values_json,revision FROM plugin_configuration WHERE plugin_id=?1",
            [plugin_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        Some((json, revision)) => Ok((serde_json::from_str(&json)?, revision)),
        None => Ok((BTreeMap::new(), 1)),
    }
}

pub(crate) fn configured(
    connection: &rusqlite::Connection,
    plugin_id: &str,
) -> anyhow::Result<bool> {
    let (values, revision) = stored(connection, plugin_id)?;
    Ok(project(&declarations(connection, plugin_id)?, &values, revision).configured)
}

impl HostRegistry {
    pub fn finish_plugin_configuration_stop(&self, plugin_id: &str) -> anyhow::Result<()> {
        self.finish_plugin_runtime_restart(plugin_id)
    }
    pub fn plugin_contributions(&self, plugin_id: &str) -> anyhow::Result<PluginContributions> {
        declarations(
            &self.connection.lock().unwrap_or_else(|p| p.into_inner()),
            plugin_id,
        )
    }
    pub fn plugin_configuration(&self, plugin_id: &str) -> anyhow::Result<PluginConfiguration> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let (values, revision) = stored(&connection, plugin_id)?;
        Ok(project(
            &declarations(&connection, plugin_id)?,
            &values,
            revision,
        ))
    }
    pub fn plugin_startup_configuration(
        &self,
        plugin_id: &str,
    ) -> anyhow::Result<BTreeMap<String, String>> {
        let config = self.plugin_configuration(plugin_id)?;
        anyhow::ensure!(config.configured, "plugin-configuration-required");
        Ok(config.values)
    }
    pub fn plan_plugin_configuration(
        &self,
        plugin_id: &str,
        values: &BTreeMap<String, Option<String>>,
        expected_revision: u64,
    ) -> anyhow::Result<PluginConfigurationPlan> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let tx = connection.transaction()?;
        let declaration = declarations(&tx, plugin_id)?;
        let (current, revision) = stored(&tx, plugin_id)?;
        anyhow::ensure!(
            revision == expected_revision,
            "plugin-configuration-revision-conflict"
        );
        let mut requested = current.clone();
        let normalized = declaration.normalize_values(values)?;
        let current_projection = project(&declaration, &current, revision);
        for key in values.keys() {
            anyhow::ensure!(
                !current_projection.locked_fields.contains(key),
                "plugin-configuration-environment-locked"
            );
            match normalized.get(key) {
                Some(value) => {
                    requested.insert(key.clone(), value.clone());
                }
                None => {
                    requested.remove(key);
                }
            }
        }
        let next = project(&declaration, &requested, revision);
        // Missing required values is a supported configuration state; invalid values never are.
        anyhow::ensure!(
            next.error_code.as_deref() != Some("invalid-plugin-configuration"),
            "invalid-plugin-configuration"
        );
        let effective_changed = current_projection.values != next.values;
        let affected_libraries: u64 = tx.query_row(
            "SELECT COUNT(*) FROM library_sync_bindings WHERE plugin_id=?1",
            [plugin_id],
            |r| r.get(0),
        )?;
        let authenticated:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM plugin_accounts WHERE plugin_id=?1 AND auth_state='authenticated')",[plugin_id],|r|r.get(0))?;
        let epoch: i64 = tx.query_row(
            "SELECT lifecycle_epoch FROM plugin_installations WHERE plugin_id=?1",
            [plugin_id],
            |r| r.get(0),
        )?;
        let now = unix_timestamp_millis();
        let expires_at = now + 5 * 60 * 1000;
        let plan_uuid = uuid::Uuid::now_v7().to_string();
        let requires_confirmation = effective_changed && (authenticated || affected_libraries > 0);
        tx.execute(
            "DELETE FROM plugin_configuration_plans WHERE expires_at<?1",
            [now],
        )?;
        tx.execute("INSERT INTO plugin_configuration_plans(plan_uuid,plugin_id,expected_revision,requested_json,effective_json,declaration_json,lifecycle_epoch,effective_changed,affected_libraries,requires_confirmation,expires_at,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![plan_uuid,plugin_id,revision,serde_json::to_string(&requested)?,serde_json::to_string(&(&next.values,&next.locked_fields))?,serde_json::to_string(&declaration)?,epoch,effective_changed,affected_libraries,requires_confirmation,expires_at,now])?;
        tx.commit()?;
        Ok(PluginConfigurationPlan {
            plan_uuid,
            expected_revision,
            effective_changed,
            affected_libraries,
            requires_confirmation,
            expires_at,
        })
    }
    pub fn commit_plugin_configuration(
        &self,
        plugin_id: &str,
        plan_uuid: &str,
        expected_revision: u64,
        confirmed: bool,
    ) -> anyhow::Result<PluginConfigurationCommit> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let tx = connection.transaction()?;
        let row:(u64,String,String,String,i64,bool,bool,i64)=tx.query_row("SELECT expected_revision,requested_json,effective_json,declaration_json,lifecycle_epoch,effective_changed,requires_confirmation,expires_at FROM plugin_configuration_plans WHERE plan_uuid=?1 AND plugin_id=?2",params![plan_uuid,plugin_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?.ok_or_else(||anyhow::anyhow!("plugin-configuration-plan-invalid"))?;
        let (
            plan_revision,
            requested_json,
            effective_json,
            declaration_json,
            epoch,
            changed,
            requires_confirmation,
            expires_at,
        ) = row;
        let now = unix_timestamp_millis();
        anyhow::ensure!(expires_at >= now, "plugin-configuration-plan-expired");
        let (_, revision) = stored(&tx, plugin_id)?;
        anyhow::ensure!(
            revision == expected_revision && plan_revision == expected_revision,
            "plugin-configuration-revision-conflict"
        );
        let live_epoch: i64 = tx.query_row(
            "SELECT lifecycle_epoch FROM plugin_installations WHERE plugin_id=?1",
            [plugin_id],
            |r| r.get(0),
        )?;
        let declaration = declarations(&tx, plugin_id)?;
        anyhow::ensure!(
            live_epoch == epoch && serde_json::to_string(&declaration)? == declaration_json,
            "plugin-configuration-plan-stale"
        );
        let requested: BTreeMap<String, String> = serde_json::from_str(&requested_json)?;
        let next = project(&declaration, &requested, revision);
        anyhow::ensure!(
            serde_json::to_string(&(&next.values, &next.locked_fields))? == effective_json
                && next.error_code.as_deref() != Some("invalid-plugin-configuration"),
            "plugin-configuration-plan-stale"
        );
        let affected:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM library_sync_bindings WHERE plugin_id=?1) OR EXISTS(SELECT 1 FROM plugin_accounts WHERE plugin_id=?1 AND auth_state='authenticated')",[plugin_id],|r|r.get(0))?;
        anyhow::ensure!(
            !(requires_confirmation || (changed && affected)) || confirmed,
            "plugin-configuration-confirmation-required"
        );
        let fences = if changed {
            crate::host_registry::retire_plugin_auth_rollback(&tx, plugin_id, now)?;
            let fences = pause_plugin_bindings_in_transaction(&tx, plugin_id)?;
            crate::host_registry::invalidate_plugin_remote_scope_in_transaction(&tx, plugin_id)?;
            let account: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT account_uuid,secret_ref FROM plugin_accounts WHERE plugin_id=?1",
                    [plugin_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((account, reference)) = account {
                enqueue_plugin_secret_retirements(
                    &tx,
                    plugin_id,
                    &account,
                    reference.as_deref(),
                    now,
                )?;
            }
            tx.execute("INSERT INTO plugin_configuration(plugin_id,values_json,revision,updated_at) VALUES (?1,?2,?3,?4) ON CONFLICT(plugin_id) DO UPDATE SET values_json=excluded.values_json,revision=excluded.revision,updated_at=excluded.updated_at",params![plugin_id,requested_json,revision+1,now])?;
            tx.execute("UPDATE plugin_accounts SET secret_ref=NULL,stable_account_identity=NULL,account_kind=NULL,auth_state='authorization-required',generation=generation+1,updated_at=?1 WHERE plugin_id=?2",params![now,plugin_id])?;
            tx.execute("UPDATE plugin_installations SET lifecycle_epoch=lifecycle_epoch+1 WHERE plugin_id=?1",[plugin_id])?;
            tx.execute("UPDATE library_sync_bindings SET plugin_suspended_state=NULL,last_error_code='authorization-required' WHERE plugin_id=?1 AND state<>'unbinding'",[plugin_id])?;
            tx.execute("UPDATE plugin_runtime_state SET state=CASE WHEN (SELECT enabled_intent FROM plugin_installations i WHERE i.plugin_id=?2)=0 THEN 'disabled' WHEN ?3=0 THEN 'configuration-required' ELSE 'starting' END,stop_pending=1,pid=NULL,session_id=NULL,last_error_code=NULL,last_error_detail=NULL,updated_at=?1 WHERE plugin_id=?2",params![now,plugin_id,next.configured])?;
            Self::bump_sync_status_revision(&tx)?;
            fences
        } else {
            Vec::new()
        };
        tx.execute(
            "DELETE FROM plugin_configuration_plans WHERE plan_uuid=?1",
            [plan_uuid],
        )?;
        tx.commit()?;
        Ok(PluginConfigurationCommit {
            fences,
            effective_changed: changed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_registry::RegisterBundledPlugin;

    fn fixture() -> (HostRegistry, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "mangoo-generic-config-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let registry = HostRegistry::open(&path).unwrap();
        for (id, manifest) in [
            (
                "com.mangoo.configurable-sync",
                include_str!(
                    "../../../contracts/plugins/fixtures/valid-configurable-provider-v2.json"
                ),
            ),
            (
                "com.mangoo.fake-sync",
                include_str!("../../../bundled-plugins/com.mangoo.fake-sync/plugin.template.json"),
            ),
        ] {
            registry
                .register_bundled_plugin(RegisterBundledPlugin {
                    plugin_id: id,
                    version: "1.0.0",
                    manifest_json: manifest,
                    entry_relative_path: "bin/provider",
                    entry_sha256: &[7; 32],
                    install_root: std::path::Path::new("bundled"),
                    trust_policy_id: "test",
                    requested_permissions_json: "[]",
                })
                .unwrap();
            registry
                .upsert_plugin_account(
                    &format!("{id}-account"),
                    id,
                    Some("Retained Account"),
                    "authenticated",
                )
                .unwrap();
        }
        (registry, path)
    }

    #[test]
    fn generic_configuration_two_providers_noop_change_replay_and_disabled_signout() {
        let (registry, path) = fixture();
        let id = "com.mangoo.fake-sync";
        registry
            .set_plugin_account_secret(id, Some("retained-secret"), "authenticated", 4)
            .unwrap();
        let before = registry.plugin_account_secret(id).unwrap().unwrap();
        let epoch = registry.plugin_lifecycle_epoch(id).unwrap();
        let config = registry.plugin_configuration(id).unwrap();
        assert!(config.configured);
        assert_eq!(config.values["workspace"], "fixture");
        assert!(
            !registry
                .plugin_configuration("com.mangoo.configurable-sync")
                .unwrap()
                .configured
        );
        let noop = registry
            .plan_plugin_configuration(
                id,
                &BTreeMap::from([("workspace".into(), Some("fixture".into()))]),
                config.revision,
            )
            .unwrap();
        assert!(!noop.effective_changed);
        assert!(!noop.requires_confirmation);
        assert!(
            !registry
                .commit_plugin_configuration(id, &noop.plan_uuid, config.revision, false)
                .unwrap()
                .effective_changed
        );
        assert_eq!(
            registry
                .plugin_account_secret(id)
                .unwrap()
                .unwrap()
                .secret_ref,
            before.secret_ref
        );
        assert_eq!(registry.plugin_lifecycle_epoch(id).unwrap(), epoch);
        assert_eq!(
            registry.plugin_configuration(id).unwrap().revision,
            config.revision
        );
        let change = registry
            .plan_plugin_configuration(
                id,
                &BTreeMap::from([("workspace".into(), Some("second-space".into()))]),
                config.revision,
            )
            .unwrap();
        assert!(change.requires_confirmation);
        assert!(registry
            .commit_plugin_configuration(
                "com.mangoo.configurable-sync",
                &change.plan_uuid,
                config.revision,
                true
            )
            .is_err());
        assert!(registry
            .commit_plugin_configuration(id, &change.plan_uuid, config.revision, false)
            .is_err());
        assert_eq!(
            registry
                .plugin_account_secret(id)
                .unwrap()
                .unwrap()
                .secret_ref,
            before.secret_ref
        );
        registry
            .commit_plugin_configuration(id, &change.plan_uuid, config.revision, true)
            .unwrap();
        registry.finish_plugin_configuration_stop(id).unwrap();
        assert!(registry
            .commit_plugin_configuration(id, &change.plan_uuid, config.revision, true)
            .is_err());
        assert_eq!(
            registry.host_plugin(id).unwrap().unwrap().runtime_state,
            "disabled"
        );
        let after = registry.plugin_account_secret(id).unwrap().unwrap();
        assert_eq!(after.account_uuid, before.account_uuid);
        assert_eq!(after.generation, before.generation + 1);
        assert!(after.secret_ref.is_none());
        let due = registry.due_credential_retirements(10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].secret_ref, "retained-secret");
        assert_eq!(due[0].namespace, "fake-sync");
        registry
            .sign_out_plugin_account(id, after.generation)
            .unwrap()
            .unwrap();
        assert_eq!(
            registry.host_plugin(id).unwrap().unwrap().runtime_state,
            "stopping"
        );
        registry.finish_plugin_runtime_restart(id).unwrap();
        assert_eq!(
            registry.host_plugin(id).unwrap().unwrap().runtime_state,
            "disabled"
        );
        drop(registry);
        let reopened = HostRegistry::open(&path).unwrap();
        assert_eq!(
            reopened.plugin_configuration(id).unwrap().values["workspace"],
            "second-space"
        );
        assert_eq!(
            reopened.plugin_configuration(id).unwrap().revision,
            config.revision + 1
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn generic_account_invalidation_during_switch_survives_restart_and_retires_both_refs() {
        for configure in [false, true] {
            let (registry, path) = fixture();
            let id = "com.mangoo.fake-sync";
            registry.set_plugin_intent(id, true, true).unwrap();
            registry
                .record_plugin_runtime_ready(id, "switch-session", &["sync-provider.v1".into()])
                .unwrap();
            registry
                .set_plugin_account_secret(id, Some("prior-primary"), "authenticated", 4)
                .unwrap();
            assert!(registry
                .begin_plugin_auth_flow(id, "pending-switch", "switch-session")
                .unwrap());
            let launch = registry
                .plugin_runtime_launches()
                .unwrap()
                .into_iter()
                .find(|launch| launch.plugin_id == id)
                .unwrap();
            assert!(registry
                .set_plugin_account_secret_for_launch(
                    id,
                    &launch.fingerprint(),
                    Some("candidate-primary"),
                    "authenticated",
                )
                .unwrap());
            assert!(registry.due_credential_retirements(10).unwrap().is_empty());

            if configure {
                let config = registry.plugin_configuration(id).unwrap();
                let plan = registry
                    .plan_plugin_configuration(
                        id,
                        &BTreeMap::from([("workspace".into(), Some("changed-space".into()))]),
                        config.revision,
                    )
                    .unwrap();
                registry
                    .commit_plugin_configuration(id, &plan.plan_uuid, config.revision, true)
                    .unwrap();
            } else {
                registry.sign_out_plugin_account(id, 4).unwrap().unwrap();
            }
            let retired = registry.due_credential_retirements(10).unwrap();
            assert_eq!(
                retired.len(),
                2,
                "both account-switch credentials must retire"
            );
            assert!(retired
                .iter()
                .any(|entry| entry.secret_ref == "prior-primary"));
            assert!(retired
                .iter()
                .any(|entry| entry.secret_ref == "candidate-primary"));
            assert!(registry.plugin_auth_flow_ownership(id).unwrap().is_none());
            // Restart before the route reaches asynchronous runtime stop/flow cleanup.
            drop(registry);
            let reopened = HostRegistry::open(&path).unwrap();
            reopened.recover_plugin_auth_flows_for_startup().unwrap();
            let account = reopened.plugin_account_secret(id).unwrap().unwrap();
            assert!(account.secret_ref.is_none());
            assert_eq!(
                reopened
                    .host_plugin(id)
                    .unwrap()
                    .unwrap()
                    .auth_state
                    .as_deref(),
                Some(if configure {
                    "authorization-required"
                } else {
                    "signed-out"
                })
            );
            assert_eq!(reopened.due_credential_retirements(10).unwrap().len(), 2);
            drop(reopened);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn generic_configuration_rejects_changed_epoch_declaration_and_environment() {
        let (registry, path) = fixture();
        let id = "com.mangoo.fake-sync";
        let initial = registry.plugin_configuration(id).unwrap();
        let request = BTreeMap::from([("workspace".into(), Some("changed".into()))]);
        let expired = registry
            .plan_plugin_configuration(id, &request, initial.revision)
            .unwrap();
        registry
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE plugin_configuration_plans SET expires_at=0 WHERE plan_uuid=?1",
                [&expired.plan_uuid],
            )
            .unwrap();
        assert_eq!(
            registry
                .commit_plugin_configuration(id, &expired.plan_uuid, initial.revision, true)
                .unwrap_err()
                .to_string(),
            "plugin-configuration-plan-expired"
        );
        let plan = registry
            .plan_plugin_configuration(id, &request, initial.revision)
            .unwrap();
        registry.connection.lock().unwrap().execute("UPDATE plugin_installations SET lifecycle_epoch=lifecycle_epoch+1 WHERE plugin_id=?1",[id]).unwrap();
        assert!(registry
            .commit_plugin_configuration(id, &plan.plan_uuid, initial.revision, true)
            .is_err());
        let plan = registry
            .plan_plugin_configuration(id, &request, initial.revision)
            .unwrap();
        registry.connection.lock().unwrap().execute("UPDATE plugin_versions SET manifest_json=json_set(manifest_json,'$.contributions.configuration[0].label','Changed label') WHERE plugin_id=?1",[id]).unwrap();
        assert!(registry
            .commit_plugin_configuration(id, &plan.plan_uuid, initial.revision, true)
            .is_err());
        assert_eq!(
            registry.plugin_configuration(id).unwrap().values["workspace"],
            "fixture"
        );
        assert!(registry
            .plan_plugin_configuration(
                id,
                &BTreeMap::from([("unknown".into(), Some("x".into()))]),
                initial.revision
            )
            .is_err());
        // A newly supplied environment lock invalidates the plan even when its value is equal.
        let environment = "MANGOO_GENERIC_PLAN_LOCK";
        registry.connection.lock().unwrap().execute("UPDATE plugin_versions SET manifest_json=json_set(manifest_json,'$.contributions.configuration[0].environment',?2) WHERE plugin_id=?1",params![id, environment]).unwrap();
        let plan = registry
            .plan_plugin_configuration(id, &request, initial.revision)
            .unwrap();
        std::env::set_var(environment, "changed");
        assert_eq!(
            registry
                .plan_plugin_configuration(id, &request, initial.revision)
                .unwrap_err()
                .to_string(),
            "plugin-configuration-environment-locked"
        );
        let commit =
            registry.commit_plugin_configuration(id, &plan.plan_uuid, initial.revision, true);
        std::env::remove_var(environment);
        assert_eq!(
            commit.unwrap_err().to_string(),
            "plugin-configuration-plan-stale"
        );
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn generic_configuration_multifield_environment_lock_is_field_scoped() {
        let mut declaration: PluginContributions = serde_json::from_value(
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../../bundled-plugins/com.mangoo.fake-sync/plugin.template.json"
            ))
            .unwrap()["contributions"]
                .clone(),
        )
        .unwrap();
        let mut second = declaration.configuration[0].clone();
        second.id = "region".into();
        second.environment = Some("MANGOO_GENERIC_FIXTURE_REGION".into());
        second.default_value = Some("west".into());
        declaration.configuration = vec![declaration.configuration[0].clone(), second];
        std::env::set_var("MANGOO_GENERIC_FIXTURE_REGION", "east");
        let config = project(&declaration, &BTreeMap::new(), 7);
        std::env::remove_var("MANGOO_GENERIC_FIXTURE_REGION");
        assert!(config.configured);
        assert!(!config.read_only);
        assert_eq!(config.source, "environment");
        assert_eq!(config.locked_fields, vec!["region"]);
        assert_eq!(config.values["region"], "east");
        assert_eq!(config.values["workspace"], "fixture");
    }

    #[test]
    fn generic_primary_credential_recovery_preserves_adopted_and_retires_unadopted() {
        let (registry, path) = fixture();
        let id = "com.mangoo.fake-sync";
        let account = format!("{id}-account");
        registry.set_plugin_intent(id, true, true).unwrap();
        registry
            .record_plugin_runtime_ready(id, "credential-session", &["sync-provider.v1".into()])
            .unwrap();
        let fingerprint = registry
            .plugin_runtime_launches()
            .unwrap()
            .into_iter()
            .find(|l| l.plugin_id == id)
            .unwrap()
            .fingerprint();
        let primary = registry
            .plugin_contributions(id)
            .unwrap()
            .primary_credential
            .unwrap();
        for reference in ["adopted-primary", "unadopted-primary"] {
            assert!(registry
                .register_plugin_secret_handle_for_launch(
                    id,
                    &account,
                    &fingerprint,
                    reference,
                    &primary.namespace,
                    &primary.purpose
                )
                .unwrap());
        }
        assert!(registry
            .set_plugin_account_secret_for_launch(
                id,
                &fingerprint,
                Some("adopted-primary"),
                "authenticated"
            )
            .unwrap());
        assert!(registry.due_credential_retirements(10).unwrap().is_empty());
        drop(registry);
        let registry = HostRegistry::open(&path).unwrap();
        let retirements = registry.due_credential_retirements(10).unwrap();
        assert_eq!(retirements.len(), 1);
        assert_eq!(retirements[0].secret_ref, "unadopted-primary");
        assert_eq!(retirements[0].namespace, primary.namespace);
        assert_eq!(
            registry
                .plugin_account_secret(id)
                .unwrap()
                .unwrap()
                .secret_ref
                .as_deref(),
            Some("adopted-primary")
        );
        registry
            .complete_credential_retirement(&retirements[0].retirement_uuid)
            .unwrap();
        drop(registry);
        let registry = HostRegistry::open(&path).unwrap();
        assert!(registry.due_credential_retirements(10).unwrap().is_empty());
        drop(registry);
        let _ = std::fs::remove_file(path);
    }
}
