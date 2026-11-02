use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};

use super::rpc::ReverseRpcHandler;
use crate::plugins::secret_store::{SecretScope, SecretStore};

pub struct SecretBroker {
    store: Arc<dyn SecretStore>,
    registry: Arc<crate::host_registry::HostRegistry>,
    plugin_id: String,
    account_uuid: String,
    launch_fingerprint: String,
}

impl SecretBroker {
    pub fn new(
        store: Arc<dyn SecretStore>,
        registry: Arc<crate::host_registry::HostRegistry>,
        plugin_id: String,
        account_uuid: String,
        launch_fingerprint: String,
    ) -> Self {
        Self {
            store,
            registry,
            plugin_id,
            account_uuid,
            launch_fingerprint,
        }
    }
    fn scope(&self, payload: &Value, operation: &str) -> Result<SecretScope, String> {
        if !self
            .registry
            .plugin_runtime_launch_is_current(&self.plugin_id, &self.launch_fingerprint)
            .map_err(|e| e.to_string())?
        {
            return Err("plugin runtime session is stale".into());
        }
        let namespace = payload
            .get("namespace")
            .and_then(Value::as_str)
            .ok_or("missing namespace")?;
        let purpose = payload
            .get("purpose")
            .and_then(Value::as_str)
            .ok_or("missing purpose")?;
        let projection = self
            .registry
            .host_plugin(&self.plugin_id)
            .map_err(|e| e.to_string())?
            .ok_or("plugin is not registered")?;
        let allowed = projection
            .granted_permissions
            .as_array()
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("kind").and_then(Value::as_str) == Some("secret")
                        && item.get("namespace").and_then(Value::as_str) == Some(namespace)
                        && item
                            .get("operations")
                            .and_then(Value::as_array)
                            .is_some_and(|operations| {
                                operations
                                    .iter()
                                    .any(|value| value.as_str() == Some(operation))
                            })
                })
            });
        if !allowed {
            return Err("secret namespace is not granted".into());
        }
        Ok(SecretScope {
            plugin_id: self.plugin_id.clone(),
            account_uuid: self.account_uuid.clone(),
            namespace: namespace.into(),
            purpose: purpose.into(),
        })
    }
}

impl ReverseRpcHandler for SecretBroker {
    fn handle(&self, method: &str, payload: &Value) -> Result<Value, String> {
        let operation = match method {
            "host.secret.current" | "host.secret.get" => "read",
            "host.secret.create"
            | "host.secret.cas"
            | "host.secret.create_scoped"
            | "host.secret.cas_scoped" => "write",
            "host.secret.delete"
            | "host.secret.delete_scoped"
            | "host.auth.authorization_required" => "delete",
            _ => return Err("reverse RPC method is not allowed".into()),
        };
        let scope = self.scope(payload, operation)?;
        if matches!(
            method,
            "host.secret.current"
                | "host.secret.create"
                | "host.secret.cas"
                | "host.secret.delete"
                | "host.auth.authorization_required"
        ) {
            let declaration = self
                .registry
                .plugin_contributions(&self.plugin_id)
                .map_err(|e| e.to_string())?;
            if !declaration
                .primary_credential
                .as_ref()
                .is_some_and(|primary| {
                    primary.namespace == scope.namespace && primary.purpose == scope.purpose
                })
            {
                return Err("primary credential scope is not declared".into());
            }
        }
        let decode = |key: &str| -> Result<Vec<u8>, String> {
            base64::engine::general_purpose::STANDARD
                .decode(
                    payload
                        .get(key)
                        .and_then(Value::as_str)
                        .ok_or("missing value")?,
                )
                .map_err(|_| "invalid base64 value".into())
        };
        match method {
            "host.secret.current" => {
                let account = self
                    .registry
                    .plugin_account_secret(&self.plugin_id)
                    .map_err(|e| e.to_string())?
                    .ok_or("plugin account is not registered")?;
                if account.account_uuid != self.account_uuid {
                    return Err("plugin account does not match runtime".into());
                }
                Ok(json!({"secretRef":account.secret_ref,"accountGeneration":account.generation}))
            }
            "host.secret.create" => {
                let value = decode("value")?;
                if !self
                    .registry
                    .plugin_auth_secret_write_is_allowed(&self.plugin_id)
                    .map_err(|e| e.to_string())?
                {
                    return Err("OAuth flow is cancelling".into());
                }
                let reference = crate::plugins::secret_store::allocate_secret_ref(&scope)
                    .map_err(|e| e.to_string())?;
                if !self
                    .registry
                    .reserve_plugin_secret_handle_for_launch(
                        &self.plugin_id,
                        &self.account_uuid,
                        &self.launch_fingerprint,
                        &reference,
                        &scope.namespace,
                        &scope.purpose,
                    )
                    .map_err(|e| e.to_string())?
                {
                    return Err("plugin runtime session is stale".into());
                }
                if let Err(error) = self.store.create_with_ref(&scope, &reference, &value) {
                    let cleaned = matches!(
                        self.store.delete(&scope, &reference),
                        Ok(()) | Err(crate::plugins::secret_store::SecretStoreError::NotFound)
                    );
                    self.registry
                        .abandon_plugin_secret_creation(
                            &self.plugin_id,
                            &self.account_uuid,
                            &reference,
                            !cleaned,
                        )
                        .map_err(|e| e.to_string())?;
                    return Err(error.to_string());
                }
                if !self
                    .registry
                    .finalize_plugin_secret_handle_for_launch(
                        &self.plugin_id,
                        &self.account_uuid,
                        &self.launch_fingerprint,
                        &reference,
                    )
                    .map_err(|e| e.to_string())?
                {
                    let cleaned = matches!(
                        self.store.delete(&scope, &reference),
                        Ok(()) | Err(crate::plugins::secret_store::SecretStoreError::NotFound)
                    );
                    self.registry
                        .abandon_plugin_secret_creation(
                            &self.plugin_id,
                            &self.account_uuid,
                            &reference,
                            !cleaned,
                        )
                        .map_err(|e| e.to_string())?;
                    return Err("plugin runtime session is stale".into());
                }
                if !self
                    .registry
                    .set_plugin_account_secret_for_launch(
                        &self.plugin_id,
                        &self.launch_fingerprint,
                        Some(&reference),
                        "authenticated",
                    )
                    .map_err(|e| e.to_string())?
                {
                    let cleaned = matches!(
                        self.store.delete(&scope, &reference),
                        Ok(()) | Err(crate::plugins::secret_store::SecretStoreError::NotFound)
                    );
                    self.registry
                        .abandon_plugin_secret_creation(
                            &self.plugin_id,
                            &self.account_uuid,
                            &reference,
                            !cleaned,
                        )
                        .map_err(|e| e.to_string())?;
                    return Err("plugin runtime session is stale".into());
                }
                Ok(json!({"secretRef":reference,"generation":1}))
            }
            "host.secret.create_scoped" => {
                let value = decode("value")?;
                let reference = crate::plugins::secret_store::allocate_secret_ref(&scope)
                    .map_err(|e| e.to_string())?;
                if !self
                    .registry
                    .reserve_plugin_secret_handle_for_launch(
                        &self.plugin_id,
                        &self.account_uuid,
                        &self.launch_fingerprint,
                        &reference,
                        &scope.namespace,
                        &scope.purpose,
                    )
                    .map_err(|e| e.to_string())?
                {
                    return Err("plugin runtime session is stale".into());
                }
                if let Err(error) = self.store.create_with_ref(&scope, &reference, &value) {
                    let cleaned = matches!(
                        self.store.delete(&scope, &reference),
                        Ok(()) | Err(crate::plugins::secret_store::SecretStoreError::NotFound)
                    );
                    self.registry
                        .abandon_plugin_secret_creation(
                            &self.plugin_id,
                            &self.account_uuid,
                            &reference,
                            !cleaned,
                        )
                        .map_err(|e| e.to_string())?;
                    return Err(error.to_string());
                }
                if !self
                    .registry
                    .finalize_plugin_secret_handle_for_launch(
                        &self.plugin_id,
                        &self.account_uuid,
                        &self.launch_fingerprint,
                        &reference,
                    )
                    .map_err(|e| e.to_string())?
                {
                    let cleaned = matches!(
                        self.store.delete(&scope, &reference),
                        Ok(()) | Err(crate::plugins::secret_store::SecretStoreError::NotFound)
                    );
                    self.registry
                        .abandon_plugin_secret_creation(
                            &self.plugin_id,
                            &self.account_uuid,
                            &reference,
                            !cleaned,
                        )
                        .map_err(|e| e.to_string())?;
                    return Err("plugin runtime session is stale".into());
                }
                Ok(json!({"secretRef":reference,"generation":1}))
            }
            "host.secret.get" => {
                let value = self
                    .store
                    .read(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(
                    json!({"value":base64::engine::general_purpose::STANDARD.encode(value.bytes),"generation":value.generation}),
                )
            }
            "host.secret.cas" => {
                if !self
                    .registry
                    .plugin_auth_secret_write_is_allowed(&self.plugin_id)
                    .map_err(|e| e.to_string())?
                {
                    return Err("OAuth flow is cancelling".into());
                }
                let next = self
                    .store
                    .compare_and_swap(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                        payload
                            .get("expectedGeneration")
                            .and_then(Value::as_u64)
                            .ok_or("missing generation")?,
                        &decode("value")?,
                    )
                    .map_err(|e| e.to_string())?;
                if !self
                    .registry
                    .set_plugin_account_secret_for_launch(
                        &self.plugin_id,
                        &self.launch_fingerprint,
                        payload.get("secretRef").and_then(Value::as_str),
                        "authenticated",
                    )
                    .map_err(|e| e.to_string())?
                {
                    return Err("plugin runtime session is stale".into());
                }
                Ok(json!({"generation":next}))
            }
            "host.secret.cas_scoped" => {
                let next = self
                    .store
                    .compare_and_swap(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                        payload
                            .get("expectedGeneration")
                            .and_then(Value::as_u64)
                            .ok_or("missing generation")?,
                        &decode("value")?,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(json!({"generation":next}))
            }
            "host.secret.delete" => {
                self.store
                    .delete(
                        &scope,
                        payload
                            .get("secretRef")
                            .and_then(Value::as_str)
                            .ok_or("missing secretRef")?,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            "host.secret.delete_scoped" => {
                let secret_ref = payload
                    .get("secretRef")
                    .and_then(Value::as_str)
                    .ok_or("missing secretRef")?;
                match self.store.delete(&scope, secret_ref) {
                    Ok(()) | Err(crate::plugins::secret_store::SecretStoreError::NotFound) => {}
                    Err(error) => return Err(error.to_string()),
                }
                self.registry
                    .forget_plugin_secret_handle(&self.plugin_id, &self.account_uuid, secret_ref)
                    .map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            "host.auth.authorization_required" => {
                if !self
                    .registry
                    .require_plugin_reauthorization_for_launch(
                        &self.plugin_id,
                        &self.launch_fingerprint,
                    )
                    .map_err(|e| e.to_string())?
                {
                    return Err("plugin runtime session is stale".into());
                }
                Ok(Value::Null)
            }
            _ => Err("reverse RPC method is not allowed".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generic_secret_broker_preserves_new_primary_and_retires_old_scopes() {
        let path = std::env::temp_dir().join(format!(
            "mangoo-generic-secret-broker-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let registry = Arc::new(crate::host_registry::HostRegistry::open(&path).unwrap());
        let template = include_str!(
            "../../../../../bundled-plugins/com.mangoo.fake-sync/plugin.template.json"
        );
        let manifest: Value = serde_json::from_str(template).unwrap();
        let id = "com.mangoo.fake-sync";
        let permissions = manifest["requestedPermissions"].to_string();
        registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: id,
                version: "1.0.0",
                manifest_json: template,
                entry_relative_path: "fixture",
                entry_sha256: &[0; 32],
                install_root: std::path::Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: &permissions,
            })
            .unwrap();
        registry.set_plugin_grants(id, &permissions, true).unwrap();
        registry
            .upsert_plugin_account("account", id, None, "signed-out")
            .unwrap();
        registry.set_plugin_intent(id, true, true).unwrap();
        registry
            .record_plugin_runtime_ready(id, "session", &["sync-provider.v1".into()])
            .unwrap();
        let launch = registry.plugin_runtime_launches().unwrap().remove(0);
        let store: Arc<dyn SecretStore> =
            Arc::new(crate::plugins::secret_store::MemorySecretStore::default());
        let broker = SecretBroker::new(
            store.clone(),
            registry.clone(),
            id.into(),
            "account".into(),
            launch.fingerprint(),
        );
        let primary = registry
            .plugin_contributions(id)
            .unwrap()
            .primary_credential
            .unwrap();
        let payload = json!({"namespace":primary.namespace,"purpose":primary.purpose,"value":"Y3JlZGVudGlhbA=="});
        let old = broker.handle("host.secret.create", &payload).unwrap()["secretRef"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut scoped = payload.clone();
        scoped["purpose"] = json!("upload-session-v1");
        assert!(broker.handle("host.secret.current", &scoped).is_err());
        let upload = broker.handle("host.secret.create_scoped", &scoped).unwrap()["secretRef"]
            .as_str()
            .unwrap()
            .to_owned();
        let fresh = broker.handle("host.secret.create", &payload).unwrap()["secretRef"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            registry
                .plugin_account_secret(id)
                .unwrap()
                .unwrap()
                .secret_ref
                .as_deref(),
            Some(fresh.as_str())
        );
        let retirements = registry.due_credential_retirements(10).unwrap();
        assert_eq!(retirements.len(), 2);
        assert!(retirements.iter().any(|r| r.secret_ref == old));
        assert!(retirements.iter().any(|r| r.secret_ref == upload));
        assert!(!retirements.iter().any(|r| r.secret_ref == fresh));
        let scope = SecretScope {
            plugin_id: id.into(),
            account_uuid: "account".into(),
            namespace: primary.namespace,
            purpose: primary.purpose,
        };
        assert_eq!(store.read(&scope, &fresh).unwrap().bytes, b"credential");
        for retirement in retirements {
            registry
                .complete_credential_retirement(&retirement.retirement_uuid)
                .unwrap();
        }
        assert!(registry
            .begin_plugin_auth_flow(id, "switch-flow", "session")
            .unwrap());
        let candidate = broker.handle("host.secret.create", &payload).unwrap()["secretRef"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(registry.due_credential_retirements(10).unwrap().is_empty());
        assert!(registry
            .claim_plugin_auth_cancellation(id, "switch-flow", "session")
            .unwrap());
        assert!(registry
            .cancel_plugin_auth_flow(id, "switch-flow", "session")
            .unwrap());
        assert_eq!(
            registry
                .plugin_account_secret(id)
                .unwrap()
                .unwrap()
                .secret_ref
                .as_deref(),
            Some(fresh.as_str())
        );
        let retirements = registry.due_credential_retirements(10).unwrap();
        assert_eq!(retirements.len(), 1);
        assert_eq!(retirements[0].secret_ref, candidate);
        assert_eq!(store.read(&scope, &fresh).unwrap().bytes, b"credential");
        registry
            .complete_credential_retirement(&retirements[0].retirement_uuid)
            .unwrap();
        assert!(registry
            .begin_plugin_auth_flow(id, "accepted-flow", "session")
            .unwrap());
        let accepted = broker.handle("host.secret.create", &payload).unwrap()["secretRef"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(registry.due_credential_retirements(10).unwrap().is_empty());
        assert!(registry
            .claim_plugin_auth_completion(id, "accepted-flow", "session")
            .unwrap());
        assert!(registry
            .apply_plugin_auth_result_for_flow(
                id,
                "accepted-flow",
                "session",
                "authenticated",
                Some("Accepted"),
                None,
                Some("stable-fixture")
            )
            .unwrap()
            .is_some());
        assert!(registry
            .finish_plugin_auth_flow(id, "accepted-flow", "session")
            .unwrap());
        let retirements = registry.due_credential_retirements(10).unwrap();
        assert_eq!(retirements.len(), 1);
        assert_eq!(retirements[0].secret_ref, fresh);
        assert_eq!(
            registry
                .plugin_account_secret(id)
                .unwrap()
                .unwrap()
                .secret_ref
                .as_deref(),
            Some(accepted.as_str())
        );
        drop(broker);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }
}
